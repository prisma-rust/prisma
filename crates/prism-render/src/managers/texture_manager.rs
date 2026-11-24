//! `RenderTextureManager` — RGBA8 textures backed by the bindless SRV table.
//!
//! The manager owns each texture's device-local image, memory and view, plus
//! the bindless slot it occupies. Construction registers a permanent
//! checkerboard fallback in slot 0, so an unregistered or not-yet-uploaded
//! handle can never read an unbound descriptor. The shader path checks
//! `TextureHandle::INVALID` and returns the fallback colour; the CPU-side
//! `get_srv` always returns a real slot.
//!
//! P0 scope (submission 3):
//! - `RenderTextureManager::new` builds no-bind table and registers fallback view in slot 0.
//! - `register` accepts CPU-side texture and records no-bind slot bit.
//!   Actual Vulkan image / view built by renderer in submission 9 (with per-frame command pool and graphics queue),
//!   result `ImageView` attached via `attach_image_view` here.
//!   This separation keeps manager Vulkan-independent, submission 3 compiles and unit-tests without staging buffer / barrier code.
//!
//! Textures are uploaded end to end (image + memory + view + bindless write in
//! one call) through `buffer::create_and_upload_image`.

use anyhow::Context as _;
use ash::vk;
use slotmap::{new_key_type, SlotMap};

use crate::bindless::{BindlessTextureTable, TextureHandle};
use crate::buffer::create_and_upload_image;
use crate::context::VulkanContext;

/// Edge length of the missing-texture fallback image, in texels.
///
/// Eight keeps the checker readable when a surface maps it 1:1 and still
/// averages to a neutral mid grey when it is minified.
const FALLBACK_EDGE: u32 = 8;

/// RGBA8 texels of the missing-texture fallback: a grey/black checkerboard.
///
/// Two rows of black and two of dark grey make a 4×4 tile, which repeats
/// seamlessly because eight is a whole number of tiles.
fn fallback_pixels() -> Vec<u8> {
    const GREY: [u8; 4] = [128, 128, 128, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    let mut pixels = Vec::with_capacity((FALLBACK_EDGE * FALLBACK_EDGE * 4) as usize);
    for y in 0..FALLBACK_EDGE {
        for x in 0..FALLBACK_EDGE {
            let cell = (x / 2 + y / 2) % 2;
            pixels.extend_from_slice(if cell == 0 { &GREY } else { &BLACK });
        }
    }
    pixels
}

// Local handle. The engine layer translates asset texture handles
// into this when it calls `RenderTextureManager::reserve` so the render
// crate stays decoupled from the asset pipeline.
new_key_type! {
    /// Slotmap handle into [`RenderTextureManager`].
    pub struct TextureHandleSlot;
}

/// Backwards-compatible alias for the slotmap-typed handle. Public so
/// engine code can name it without depending on the new_key_type
/// expansion directly.
pub type AssetTextureHandle = TextureHandleSlot;

/// Plain-data texture description used at the manager boundary. The
/// engine layer translates asset texture data into this.
#[derive(Debug, Clone)]
pub struct TextureUploadInput {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    /// >1 when KTX2 already contains full mip chain, otherwise 0 means auto-derive from width/height (only Rgba8).
    pub mip_levels: u32,
    /// Tightly packed rows, no padding. When `mip_levels > 1` this may hold the
    /// whole baked chain (level 0 first, then each halved level); the upload
    /// copies every level instead of blitting it.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    Rgba8,
    /// sRGB-encoded RGBA8 -> Vulkan `R8G8B8A8_SRGB`. Hardware performs the
    /// sRGB->linear conversion on sample, so the shader receives linear values
    /// and must NOT apply a manual `pow(2.2)`. Used for albedo / emissive.
    Rgba8Srgb,
    // ── Block-compressed formats (DESIGN §7.2): offline encode, upload compressed blocks directly ──
    /// BC1 / DXT1 (RGB, 4×4 block 8 bytes).
    Bc1Unorm,
    Bc1Srgb,
    /// BC3 / DXT5 (RGBA, 4×4 block 16 bytes).
    Bc3Unorm,
    Bc3Srgb,
    Bc7Unorm,
    Bc7Srgb,
    Bc5Unorm,
    Bc4Unorm,
    Bc6HUfloat,
    Astc4x4Unorm,
    Astc4x4Srgb,
    Astc6x6Unorm,
    Astc6x6Srgb,
    Astc8x8Unorm,
    Astc8x8Srgb,
    Astc12x12Unorm,
    Astc12x12Srgb,
    Etc2R8G8B8A8Unorm,
    Etc2R8G8B8A8Srgb,
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            TextureFormat::Rgba8 | TextureFormat::Rgba8Srgb => 4,
            // Block-compressed formats approximate bytes per pixel (only for estimation, exact size uses compressed_byte_len)
            TextureFormat::Bc1Unorm | TextureFormat::Bc1Srgb => 1, // 0.5B/px, rounded up
            TextureFormat::Bc3Unorm | TextureFormat::Bc3Srgb => 1,
            TextureFormat::Bc7Unorm | TextureFormat::Bc7Srgb => 1, // 4x4 block 16B = 1B/px
            TextureFormat::Bc5Unorm => 1,
            TextureFormat::Bc4Unorm => 1, // actually 0.5B/px, rounded up for lenient validation
            TextureFormat::Bc6HUfloat => 1,
            TextureFormat::Astc4x4Unorm | TextureFormat::Astc4x4Srgb => 1, // 8 bpp
            TextureFormat::Astc6x6Unorm | TextureFormat::Astc6x6Srgb => 1, // ~3.56 bpp
            TextureFormat::Astc8x8Unorm | TextureFormat::Astc8x8Srgb => 1,
            TextureFormat::Astc12x12Unorm | TextureFormat::Astc12x12Srgb => 1,
            TextureFormat::Etc2R8G8B8A8Unorm | TextureFormat::Etc2R8G8B8A8Srgb => 1,
        }
    }

    pub const fn is_compressed(self) -> bool {
        !matches!(self, TextureFormat::Rgba8 | TextureFormat::Rgba8Srgb)
    }

    /// Exact compressed block byte count (by Vulkan block size).
    pub fn compressed_byte_len(self, width: u32, height: u32) -> usize {
        match self {
            TextureFormat::Rgba8 | TextureFormat::Rgba8Srgb => {
                (width as usize) * (height as usize) * 4
            }
            TextureFormat::Bc3Unorm
            | TextureFormat::Bc3Srgb
            | TextureFormat::Bc7Unorm
            | TextureFormat::Bc7Srgb
            | TextureFormat::Bc5Unorm
            | TextureFormat::Bc6HUfloat => {
                let bw = width.div_ceil(4);
                let bh = height.div_ceil(4);
                (bw as usize) * (bh as usize) * 16
            }
            TextureFormat::Bc4Unorm | TextureFormat::Bc1Unorm | TextureFormat::Bc1Srgb => {
                let bw = width.div_ceil(4);
                let bh = height.div_ceil(4);
                (bw as usize) * (bh as usize) * 8
            }
            TextureFormat::Astc4x4Unorm | TextureFormat::Astc4x4Srgb => {
                let bw = width.div_ceil(4);
                let bh = height.div_ceil(4);
                (bw as usize) * (bh as usize) * 16
            }
            TextureFormat::Astc6x6Unorm | TextureFormat::Astc6x6Srgb => {
                let bw = width.div_ceil(6);
                let bh = height.div_ceil(6);
                (bw as usize) * (bh as usize) * 16
            }
            TextureFormat::Astc8x8Unorm | TextureFormat::Astc8x8Srgb => {
                let bw = width.div_ceil(8);
                let bh = height.div_ceil(8);
                (bw as usize) * (bh as usize) * 16
            }
            TextureFormat::Astc12x12Unorm | TextureFormat::Astc12x12Srgb => {
                let bw = width.div_ceil(12);
                let bh = height.div_ceil(12);
                (bw as usize) * (bh as usize) * 16
            }
            TextureFormat::Etc2R8G8B8A8Unorm | TextureFormat::Etc2R8G8B8A8Srgb => {
                let bw = width.div_ceil(4);
                let bh = height.div_ceil(4);
                (bw as usize) * (bh as usize) * 16
            }
        }
    }

    /// The Vulkan image format to use for this texture kind.
    pub const fn vk_format(self) -> vk::Format {
        match self {
            TextureFormat::Rgba8 => vk::Format::R8G8B8A8_UNORM,
            TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
            TextureFormat::Bc1Unorm => vk::Format::BC1_RGBA_UNORM_BLOCK,
            TextureFormat::Bc1Srgb => vk::Format::BC1_RGBA_SRGB_BLOCK,
            TextureFormat::Bc3Unorm => vk::Format::BC3_UNORM_BLOCK,
            TextureFormat::Bc3Srgb => vk::Format::BC3_SRGB_BLOCK,
            TextureFormat::Bc7Unorm => vk::Format::BC7_UNORM_BLOCK,
            TextureFormat::Bc7Srgb => vk::Format::BC7_SRGB_BLOCK,
            TextureFormat::Bc5Unorm => vk::Format::BC5_UNORM_BLOCK,
            TextureFormat::Bc4Unorm => vk::Format::BC4_UNORM_BLOCK,
            TextureFormat::Bc6HUfloat => vk::Format::BC6H_UFLOAT_BLOCK,
            TextureFormat::Astc4x4Unorm => vk::Format::ASTC_4X4_UNORM_BLOCK,
            TextureFormat::Astc4x4Srgb => vk::Format::ASTC_4X4_SRGB_BLOCK,
            TextureFormat::Astc6x6Unorm => vk::Format::ASTC_6X6_UNORM_BLOCK,
            TextureFormat::Astc6x6Srgb => vk::Format::ASTC_6X6_SRGB_BLOCK,
            TextureFormat::Astc8x8Unorm => vk::Format::ASTC_8X8_UNORM_BLOCK,
            TextureFormat::Astc8x8Srgb => vk::Format::ASTC_8X8_SRGB_BLOCK,
            TextureFormat::Astc12x12Unorm => vk::Format::ASTC_12X12_UNORM_BLOCK,
            TextureFormat::Astc12x12Srgb => vk::Format::ASTC_12X12_SRGB_BLOCK,
            TextureFormat::Etc2R8G8B8A8Unorm => vk::Format::ETC2_R8G8B8A8_UNORM_BLOCK,
            TextureFormat::Etc2R8G8B8A8Srgb => vk::Format::ETC2_R8G8B8A8_SRGB_BLOCK,
        }
    }
}

/// Whether `pixels` is an acceptable payload for this upload.
///
/// Compressed formats and any upload that claims more than one mip level are
/// allowed to carry a *larger* buffer than the base level — that is how a
/// pre-baked KTX2 chain arrives. Everything else must be exactly the base
/// level, so a plainly wrong buffer is still rejected.
fn accepts_pixel_len(
    format: TextureFormat,
    mip_levels: u32,
    pixels_len: usize,
    width: u32,
    height: u32,
) -> bool {
    let base = if format.is_compressed() {
        format.compressed_byte_len(width, height)
    } else {
        (width as usize) * (height as usize) * format.bytes_per_pixel()
    };
    if format.is_compressed() || mip_levels > 1 {
        pixels_len >= base
    } else {
        pixels_len == base
    }
}

/// A handle into the texture manager plus the bindless SRV slot assigned
/// to it. The GPU image / memory / view are owned by the manager and freed
/// in `destroy`.
pub struct UploadedTexture {
    pub srv: TextureHandle,
    /// Width/height are stored here so the renderer can build the image
    /// view info without keeping the input around.
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    /// Owned GPU objects. Kept so `destroy` can release them; the bindless
    /// SRV descriptor merely references `view`.
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
}

/// Manager of GPU textures. Owns the [`BindlessTextureTable`] and the
/// bindless slot for every registered texture.
pub struct RenderTextureManager {
    bindless: BindlessTextureTable,
    textures: SlotMap<AssetTextureHandle, UploadedTexture>,
    /// Slot 0 of the bindless table is reserved for the missing-texture
    /// fallback and is never reallocated.
    fallback_srv: TextureHandle,
    /// Slot 1 of the bindless table is reserved for the built-in 1x1 pure-white
    /// texture and is never reallocated.
    white_srv: TextureHandle,
    /// The fallback image view — written into any slot that is released
    /// via `unregister` so a stale handle sampling the freed view hits the
    /// checkerboard instead of a use-after-free.
    fallback_view: vk::ImageView,
    /// Total slots the bindless table can hold. User textures start at
    /// slot 2 (slot 0 is the fallback, slot 1 is 1x1 white).
    user_capacity: u32,
    destroyed: bool,
}

impl RenderTextureManager {
    /// Construct a new manager with the checkerboard fallback in slot 0 and
    /// a built-in 1x1 white texture in slot 1 of the bindless table.
    ///
    /// `user_capacity` is the maximum number of user textures the manager
    /// will accept; the two built-in slots are allocated *in addition* to this.
    pub fn new(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        user_capacity: u32,
    ) -> anyhow::Result<Self> {
        let total = user_capacity + 2;
        let mut bindless = BindlessTextureTable::new(&context.device, total)
            .map_err(|e| anyhow::anyhow!("RenderTextureManager::new: bindless: {e}"))?;

        // Missing-texture fallback: a neutral checkerboard, written into
        // bindless slot 0. It marks "nothing is bound here" without shouting:
        // the pattern is recognisable at any scale, and a 1×1 sample of it is
        // just mid grey, so a surface that samples one texel reads as
        // untextured rather than as an error.
        let (fb_image, fb_memory, fb_view) = unsafe {
            create_and_upload_image(
                context,
                command_pool,
                graphics_queue,
                FALLBACK_EDGE,
                FALLBACK_EDGE,
                &fallback_pixels(),
                1,
                vk::Format::R8G8B8A8_UNORM,
            )
        }
        .context("RenderTextureManager::new: create checkerboard fallback")?;
        bindless
            .register_with_handle(TextureHandle::FALLBACK.0, fb_view)
            .context("RenderTextureManager::new: register fallback in slot 0")?;
        let fallback_srv = TextureHandle::FALLBACK;
        // Keep the fallback GPU objects alive for the manager's lifetime.
        let fallback_tex = UploadedTexture {
            srv: fallback_srv,
            width: FALLBACK_EDGE,
            height: FALLBACK_EDGE,
            mip_levels: 1,
            image: fb_image,
            memory: fb_memory,
            view: fb_view,
        };
        let fallback_view = fb_view;

        // Built-in 1x1 pure-white texture in bindless slot 1 for solid-color
        // sprites and tinted UI quads through the standard texture sampling path.
        let white_pixels = [255u8, 255, 255, 255];
        let (white_image, white_memory, white_view) = unsafe {
            create_and_upload_image(
                context,
                command_pool,
                graphics_queue,
                1,
                1,
                &white_pixels,
                1,
                vk::Format::R8G8B8A8_UNORM,
            )
        }
        .context("RenderTextureManager::new: create 1x1 white texture")?;
        bindless
            .register_with_handle(TextureHandle::WHITE.0, white_view)
            .context("RenderTextureManager::new: register 1x1 white texture in slot 1")?;
        let white_srv = TextureHandle::WHITE;
        let white_tex = UploadedTexture {
            srv: white_srv,
            width: 1,
            height: 1,
            mip_levels: 1,
            image: white_image,
            memory: white_memory,
            view: white_view,
        };

        let mut textures = SlotMap::with_key();
        // Store the built-in textures so `destroy` frees them. Slot 0 and slot 1
        // are claimed, so user textures start at slot 2.
        textures.insert(fallback_tex);
        textures.insert(white_tex);

        Ok(Self {
            bindless,
            textures,
            fallback_srv,
            white_srv,
            fallback_view,
            user_capacity,
            destroyed: false,
        })
    }

    /// The bindless SRV slot of the missing-texture fallback.
    pub fn fallback_srv(&self) -> TextureHandle {
        self.fallback_srv
    }

    /// The bindless SRV slot of the built-in 1x1 pure-white texture.
    pub fn white_srv(&self) -> TextureHandle {
        self.white_srv
    }

    /// Raw bindless table — exposed so the renderer can bind the descriptor
    /// set as part of its pipeline setup.
    pub fn bindless(&self) -> &BindlessTextureTable {
        &self.bindless
    }

    /// Mut access to the bindless table for the renderer to write fallback
    /// view / image view creation in commit 9.
    pub fn bindless_mut(&mut self) -> &mut BindlessTextureTable {
        &mut self.bindless
    }

    /// Upload a CPU-side texture to a device-local image, register its view
    /// in the bindless SRV table, and return a handle. The handle maps to
    /// the bindless slot via [`get_srv`](Self::get_srv).
    pub fn reserve(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        // Whether a block-compressed format is sampleable depends on the device
        // (many mobile GPUs lack BC, desktops lack ASTC). Checking up front
        // turns a VUID flood plus a failed upload into one explicit error that
        // the caller degrades to the fallback surface.
        if input.format.is_compressed()
            && !texture_format_supported(
                &context.instance,
                context.physical_device,
                input.format.vk_format(),
            )
        {
            anyhow::bail!(
                "device cannot sample {:?}; compressed texture rejected",
                input.format
            );
        }
        let expected = if input.format.is_compressed() {
            input.format.compressed_byte_len(input.width, input.height)
        } else {
            (input.width as usize) * (input.height as usize) * input.format.bytes_per_pixel()
        };
        // Compressed formats allow longer mip chain pixels, loose validation: at least base mip size
        let valid = accepts_pixel_len(
            input.format,
            input.mip_levels,
            input.pixels.len(),
            input.width,
            input.height,
        );
        if !valid {
            anyhow::bail!(
                "TextureUploadInput: pixel buffer size {} does not match {}x{}*{} (expected >= {})",
                input.pixels.len(),
                input.width,
                input.height,
                input.format.bytes_per_pixel(),
                expected
            );
        }
        if self.textures.len().saturating_sub(2) as u32 >= self.user_capacity {
            anyhow::bail!(
                "RenderTextureManager: user capacity {} exhausted",
                self.user_capacity
            );
        }

        // Upload pixels → VkImage + VkImageView (transferDst + SAMPLED).
        let mip_levels = if input.mip_levels != 0 {
            input.mip_levels
        } else if input.format.is_compressed() || input.width <= 1 || input.height <= 1 {
            // Compressed format mips pre-generated by KTX2; 1x1 has no mip to generate.
            1
        } else {
            (input.width.max(input.height) as f32).log2().floor() as u32 + 1
        };
        let (image, memory, view) = unsafe {
            create_and_upload_image(
                context,
                command_pool,
                graphics_queue,
                input.width,
                input.height,
                &input.pixels,
                mip_levels,
                input.format.vk_format(),
            )
        }
        .context("RenderTextureManager::reserve: upload texture")?;

        // Reserve the next bindless SRV slot (slot 0 is the fallback,
        // already taken, so this returns 1, 2, ...).
        let srv = self
            .bindless
            .register(view)
            .context("RenderTextureManager::reserve: register bindless SRV")?;

        let handle = self.textures.insert(UploadedTexture {
            srv,
            width: input.width,
            height: input.height,
            mip_levels,
            image,
            memory,
            view,
        });
        Ok(handle)
    }

    /// Like [`reserve`](Self::reserve) but records the image upload into a
    /// shared [`BatchUploader`](crate::batch::BatchUploader) so many textures
    /// can be uploaded with a single submit + fence. The caller must finish
    /// the uploader before sampling the textures.
    pub fn reserve_into(
        &mut self,
        _context: &VulkanContext,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        let expected = if input.format.is_compressed() {
            input.format.compressed_byte_len(input.width, input.height)
        } else {
            (input.width as usize) * (input.height as usize) * input.format.bytes_per_pixel()
        };
        let valid = accepts_pixel_len(
            input.format,
            input.mip_levels,
            input.pixels.len(),
            input.width,
            input.height,
        );
        if !valid {
            anyhow::bail!(
                "TextureUploadInput: pixel buffer size {} does not match {}x{}*{} (expected >= {})",
                input.pixels.len(),
                input.width,
                input.height,
                input.format.bytes_per_pixel(),
                expected
            );
        }
        if self.textures.len().saturating_sub(2) as u32 >= self.user_capacity {
            anyhow::bail!(
                "RenderTextureManager: user capacity {} exhausted",
                self.user_capacity
            );
        }

        let mip_levels = if input.mip_levels != 0 {
            input.mip_levels
        } else if input.format.is_compressed() {
            1
        } else {
            crate::batch::mip_level_count(input.width, input.height)
        };
        let (image, memory, view) = uploader
            .upload_image(
                input.width,
                input.height,
                mip_levels,
                &input.pixels,
                input.format.vk_format(),
            )
            .context("RenderTextureManager::reserve_into: upload texture")?;

        let srv = self
            .bindless
            .register(view)
            .context("RenderTextureManager::reserve_into: register bindless SRV")?;

        let handle = self.textures.insert(UploadedTexture {
            srv,
            width: input.width,
            height: input.height,
            mip_levels,
            image,
            memory,
            view,
        });
        Ok(handle)
    }

    /// Translate an asset-side texture handle to its bindless SRV slot.
    /// Returns `fallback_srv` (not `INVALID`) when the handle is unknown
    /// so shaders can always sample something visible.
    pub fn get_srv(&self, handle: AssetTextureHandle) -> TextureHandle {
        self.textures
            .get(handle)
            .map(|t| t.srv)
            .unwrap_or(self.fallback_srv)
    }

    /// Drop a single entry, free its GPU image/memory/view, and reclaim its
    /// bindless SRV slot.
    ///
    /// The slot is rewritten with the fallback view first, so a stale handle
    /// sampling it gets the checkerboard instead of a freed-view
    /// use-after-free. The slot number returns to the free-list so a later
    /// `reserve` reuses it.
    pub fn unregister(&mut self, handle: AssetTextureHandle, device: &ash::Device) {
        if let Some(tex) = self.textures.remove(handle) {
            // Rewrite the descriptor with the fallback view before the
            // image view is destroyed, so any shader holding the old
            // handle samples the fallback rather than a freed view.
            self.bindless.write_srv(tex.srv.0, self.fallback_view);
            self.bindless.release_slot(tex.srv);
            unsafe {
                device.destroy_image_view(tex.view, None);
                device.destroy_image(tex.image, None);
                device.free_memory(tex.memory, None);
            }
        }
    }

    /// Release by **bindless SRV slot** (hot reload / entity teardown).
    ///
    /// Engine-side components store only the slot (`SpriteRef.texture_slot`
    /// and friends) and cannot name the manager's `AssetTextureHandle`, so the
    /// handle is resolved here and then routed through `unregister`, which
    /// frees the image, memory, view and slot together instead of leaving them
    /// resident.
    pub fn unregister_srv(&mut self, srv: u32, device: &ash::Device) -> bool {
        if srv == self.fallback_srv.0 || srv == self.white_srv.0 {
            return false;
        }
        let Some(handle) = self
            .textures
            .iter()
            .find(|(_, tex)| tex.srv.0 == srv)
            .map(|(handle, _)| handle)
        else {
            return false;
        };
        self.unregister(handle, device);
        true
    }

    /// Release every entry (GPU image/memory/view + bindless slot). The
    /// underlying bindless table is dropped when this manager is dropped,
    /// which destroys the descriptor pool, set, layout, and 4 samplers.
    pub fn destroy(&mut self) {
        let device = self.bindless.device();
        for (_, tex) in self.textures.drain() {
            unsafe {
                device.destroy_image_view(tex.view, None);
                device.destroy_image(tex.image, None);
                device.free_memory(tex.memory, None);
            }
        }
        self.destroyed = true;
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    /// Resident texture count and estimated byte count (base mip, not mip chain; #518 Phase 2).
    pub fn stats(&self) -> (usize, u64) {
        let bytes = self
            .textures
            .values()
            .map(|texture| texture.width as u64 * texture.height as u64 * 4)
            .sum();
        (self.textures.len(), bytes)
    }
}

impl Drop for RenderTextureManager {
    fn drop(&mut self) {
        debug_assert!(
            self.destroyed || self.textures.is_empty(),
            "RenderTextureManager dropped without explicit destroy()"
        );
    }
}

/// Whether device can sample a format with `optimalTiling` (including block compression `BLOCK_COMPRESSION` bit).
///
/// `vkGetPhysicalDeviceFormatProperties` is pure query with no sync issue, so check on every upload;
/// texture upload rare enough that maintaining cache not worth it.
pub fn texture_format_supported(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
) -> bool {
    let properties =
        unsafe { instance.get_physical_device_format_properties(physical_device, format) };
    // Vulkan has no "block compressed" feature bit: whether a format can be sampled depends on its own
    // `optimalTilingFeatures` having `SAMPLED_IMAGE` (desktop generally supports BC, ASTC/ETC2
    // only on mobile GPUs).
    properties
        .optimal_tiling_features
        .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
}

#[cfg(test)]
#[path = "texture_manager_tests.rs"]
mod tests;
