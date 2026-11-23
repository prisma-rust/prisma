//! GPU part of `ui_overlay` (font atlas + pipelines + draw).
//!
//! Compiled only when `ui` feature enabled: after disabling, `ui_overlay` module keeps only input data structures
//! ([`crate::ui_overlay::UiOverlayInput`] etc.), engine fills as usual,
//! render side ignores entirely, **fontdue / ttf-parser no longer in binary**.
//!
//! Glyph bitmaps provided by rasterizer backend in `text` (`ui-fontdue` / `ui-directwrite`),
//! here only handles atlas packing, quad expansion and Vulkan draw — independent of backend.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UiVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    /// Pixel-space offset from the quad centre (rounded-rect SDF, #634).
    local: [f32; 2],
    /// Half size of the quad in pixels.
    half_size: [f32; 2],
    /// Corner radius in pixels; `< 0` = no SDF (glyph quads).
    radius: f32,
    /// Bindless SRV slot of a background image; `u32::MAX` = glyph atlas (#591).
    texture: u32,
    /// Backdrop blur radius in pixels; `0` = this quad does not sample the
    /// backdrop.
    backdrop: f32,
    /// Packed RGBA8 UNORM linear outline color for SDF text.
    outline_color: [u8; 4],
}

use std::collections::HashMap;
use std::ffi::CString;
use std::mem::size_of;

use anyhow::{Context as _, Result};
use ash::vk;

use super::text::{self, baseline_offset, TextRasterizer};
use super::{UiOverlayInput, UiTextAlign, UiTextInput};
use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::shader_bindings::ui_overlay::*;

pub(crate) const VERTICES_PER_QUAD: u32 = 6;
const VERTEX_SIZE: vk::DeviceSize = size_of::<UiVertex>() as vk::DeviceSize;

/// Descriptor slot of the filtered backdrop inside the UI's own set 0.
const BACKDROP_TEX_BINDING: u32 = 1;

// ---------------------------------------------------------------------------
// Glyph atlas
// ---------------------------------------------------------------------------

/// Atlas side length (px). R8 = 4 MB VRAM, enough for thousands of CJK + Latin SDF glyphs.
const ATLAS_SIZE: u32 = 2048;
/// Canonical pixel size used when generating TextMeshPro-style SDF glyphs.
const SDF_FONT_SIZE: f32 = 32.0;
/// Padding (in canonical pixels) around each SDF glyph for spread & outline.
const SDF_PAD: usize = 8;
/// Distance spread (in canonical pixels) mapped to `[0.0, 1.0]` (`0.5` = edge).
const SDF_SPREAD: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum GlyphKind {
    /// Direct antialiased coverage bitmap rasterized at `size_tenths` (0.1 px units).
    Bitmap(u16),
    /// Resolution-independent unit-slope SDF rasterized at `SDF_FONT_SIZE`.
    Sdf,
}

/// A cached glyph packed into the atlas (either direct bitmap or SDF).
#[derive(Clone, Copy)]
struct CachedGlyph {
    /// Rect inside atlas. Empty white glyph (w/h = 0) occupies no atlas space.
    rect: Option<[u32; 4]>,
    advance: f32,
    xmin: f32,
    ymin: f32,
}

#[derive(Clone, Copy)]
struct ScaledGlyph {
    rect: Option<[u32; 4]>,
    advance: f32,
    xmin: f32,
    ymin: f32,
    width: f32,
    height: f32,
}

/// R8 glyph atlas with glyph rasterization and line packing (host-visible, LINEAR tiling).
///
/// Atlas pixel (0,0) reserved as white — plain quad's uv=(0,0) samples alpha 1.
/// Layout kept GENERAL: host directly writes new glyphs; insert
/// HOST_WRITE → FRAGMENT_SHADER barrier in dirty frame.
///
/// `rasterizer` is **compile-time selected** text backend (see [`super::text`]);
/// `None` = this build has no text backend or no font found on system, text commands skipped.
struct GlyphAtlas {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    sampler: vk::Sampler,
    rasterizer: Option<Box<dyn TextRasterizer>>,
    /// Project font path → font id (0 reserved for system default font, #583).
    font_ids: HashMap<String, u32>,
    /// id → font path; `None` = system default font.
    font_names: Vec<Option<String>>,
    /// `(font_id, char, kind)` -> cached glyph
    glyphs: HashMap<(u32, char, GlyphKind), CachedGlyph>,
    /// Line packing cursor
    x: u32,
    y: u32,
    row_h: u32,
    /// This frame has new glyph writes → need HOST→SHADER barrier
    dirty: bool,
    /// Actual row pitch of Linear tiling (bytes). Driver may align it to larger than `ATLAS_SIZE`;
    /// writes must use this stride, cannot falsely assume `width == pitch` (#590).
    row_pitch: usize,
    device: ash::Device,
}

/// Log a sub-step of the UI overlay record that is slow enough to explain a
/// first-frame stall (lazy pipeline build, font parse, buffer growth).
fn log_slow_step(step: &str, elapsed: std::time::Duration) {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms >= 1.0 {
        log::debug!(target: "startup", "[startup] ui_overlay: {step} {ms:.3}ms");
    }
}

impl GlyphAtlas {
    fn new(context: &VulkanContext, command_pool: vk::CommandPool) -> Result<Self> {
        let device = context.device.clone();
        let (image, memory) = Self::create_image(context)?;
        let view = Self::create_view(&device, image)?;
        let sampler = Self::create_sampler(&device)?;

        // rowPitch of LINEAR tiling decided by driver, may exceed ATLAS_SIZE.
        let subresource = vk::ImageSubresource {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            array_layer: 0,
        };
        let row_pitch =
            unsafe { device.get_image_subresource_layout(image, subresource) }.row_pitch as usize;

        // UNDEFINED → GENERAL: host side directly maps and writes (LINEAR tiling).
        let cmd = allocate_temp_command_buffer(&device, command_pool)
            .context("GlyphAtlas: allocate command buffer")?;
        unsafe {
            device
                .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
                .context("GlyphAtlas: begin command buffer")?;
        }
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
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
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::HOST_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&barrier),
            );
            device.end_command_buffer(cmd)?;
        }
        submit_and_wait(&device, context.graphics_queue, command_pool, cmd)?;

        // Reserve 2x2 white pixels at (0,0) so linear sampling at uv=(0,0) yields alpha 1.
        let ptr = unsafe {
            device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .context("GlyphAtlas: map")?
        };
        unsafe {
            let base = ptr as *mut u8;
            std::ptr::write_bytes(base, 255, 2);
            std::ptr::write_bytes(base.add(row_pitch), 255, 2);
            device.unmap_memory(memory);
        }

        let font_start = std::time::Instant::now();
        let rasterizer = text::create();
        log::debug!(
            target: "startup",
            "[startup] ui_overlay: text backend + font load {:.3}ms",
            font_start.elapsed().as_secs_f64() * 1000.0
        );
        if let Some(backend) = rasterizer.as_deref() {
            if let Some(m) = backend.line_metrics(None, 16.0) {
                log::debug!(
                    "ui overlay: text metrics @16px — ascent {:.1} / descent {:.1} / line height {:.1}",
                    m.ascent,
                    m.descent,
                    m.line_height
                );
            }
        }

        Ok(Self {
            image,
            memory,
            view,
            sampler,
            rasterizer,
            font_ids: HashMap::new(),
            font_names: vec![None],
            glyphs: HashMap::new(),
            x: 3,
            y: 0,
            row_h: 3,
            dirty: false,
            row_pitch,
            device,
        })
    }

    /// Single-line text baseline offset relative to rect top (from backend line metrics).
    /// Line height (px): use `line_height` when backend provides metrics, otherwise fall back to `1.2 × font size`.
    fn line_height(&self, font: Option<&str>, font_size: f32) -> f32 {
        self.rasterizer
            .as_deref()
            .and_then(|backend| backend.line_metrics(font, font_size))
            .map(|metrics| metrics.line_height)
            .filter(|height| height.is_finite() && *height > 0.0)
            .unwrap_or(font_size * 1.2)
    }

    fn baseline_offset(&self, font: Option<&str>, font_size: f32) -> f32 {
        let metrics = self
            .rasterizer
            .as_deref()
            .and_then(|backend| backend.line_metrics(font, font_size));
        baseline_offset(metrics, font_size)
    }

    /// Font path → stable id (unregistered paths also get id, rasterizer falls back to default font).
    ///
    /// Use id rather than `String` as atlas key: `emit_text` looks up cache per glyph, cannot each frame allocate a path string
    /// for every char.
    fn font_id(&mut self, font: Option<&str>) -> u32 {
        let Some(name) = font else {
            return 0;
        };
        if let Some(id) = self.font_ids.get(name) {
            return *id;
        }
        let id = self.font_names.len() as u32;
        self.font_ids.insert(name.to_string(), id);
        self.font_names.push(Some(name.to_string()));
        id
    }

    /// Register project-bundled font; re-registering same path overwrites and drops old glyphs for that font (#583).
    fn register_font(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let Some(backend) = self.rasterizer.as_mut() else {
            return false;
        };
        if !backend.register_font(name, bytes) {
            return false;
        }
        if let Some(id) = self.font_ids.get(name).copied() {
            self.glyphs.retain(|key, _| key.0 != id);
        }
        true
    }

    fn create_image(context: &VulkanContext) -> Result<(vk::Image, vk::DeviceMemory)> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8_UNORM)
            .extent(vk::Extent3D {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::LINEAR)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { context.device.create_image(&image_info, None) }
            .context("GlyphAtlas: create image")?;
        let mem_reqs = unsafe { context.device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            &context.physical_device_memory_properties,
            mem_reqs.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        );
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_reqs.size)
            .memory_type_index(mem_type);
        let memory = unsafe { context.device.allocate_memory(&alloc, None) }
            .context("GlyphAtlas: allocate")?;
        unsafe {
            context
                .device
                .bind_image_memory(image, memory, 0)
                .context("GlyphAtlas: bind")?;
        }
        Ok((image, memory))
    }

    fn create_view(device: &ash::Device, image: vk::Image) -> Result<vk::ImageView> {
        let info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        unsafe { device.create_image_view(&info, None) }.context("GlyphAtlas: image view")
    }

    fn create_sampler(device: &ash::Device) -> Result<vk::Sampler> {
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .max_lod(0.0);
        unsafe { device.create_sampler(&info, None) }.context("GlyphAtlas: sampler")
    }

    /// Fetch a glyph as either an exact-size antialiased bitmap (`use_sdf = false`)
    /// or a resolution-independent SDF (`use_sdf = true`).
    fn glyph(
        &mut self,
        font: Option<&str>,
        ch: char,
        font_size: f32,
        use_sdf: bool,
    ) -> Option<ScaledGlyph> {
        let font_id = self.font_id(font);
        let size_tenths = (font_size * 10.0).round().clamp(10.0, 2400.0) as u16;
        let kind = if use_sdf {
            GlyphKind::Sdf
        } else {
            GlyphKind::Bitmap(size_tenths)
        };
        let key = (font_id, ch, kind);
        if !self.glyphs.contains_key(&key) {
            let name = self.font_names[font_id as usize].clone();
            let raster_size = if use_sdf {
                SDF_FONT_SIZE
            } else {
                size_tenths as f32 * 0.1
            };
            let glyph = self
                .rasterizer
                .as_mut()?
                .rasterize(name.as_deref(), ch, raster_size)?;
            let (rect, xmin, ymin) = if glyph.width > 0 && glyph.height > 0 {
                if use_sdf {
                    let (sdf, sw, sh) = bitmap_to_sdf(
                        &glyph.pixels,
                        glyph.width,
                        glyph.height,
                        SDF_PAD,
                        SDF_SPREAD,
                    );
                    let rect = self.pack(&sdf, sw, sh);
                    (
                        rect,
                        glyph.xmin as f32 - SDF_PAD as f32,
                        glyph.ymin as f32 - SDF_PAD as f32,
                    )
                } else {
                    let rect = self.pack(&glyph.pixels, glyph.width, glyph.height);
                    (rect, glyph.xmin as f32, glyph.ymin as f32)
                }
            } else {
                (None, glyph.xmin as f32, glyph.ymin as f32)
            };
            self.glyphs.insert(
                key,
                CachedGlyph {
                    rect,
                    advance: glyph.advance,
                    xmin,
                    ymin,
                },
            );
        }
        let cached = self.glyphs.get(&key)?;
        let scale = if use_sdf {
            font_size / SDF_FONT_SIZE
        } else {
            1.0
        };
        let (width, height) = cached.rect.map_or((0.0, 0.0), |[_, _, gw, gh]| {
            (gw as f32 * scale, gh as f32 * scale)
        });
        Some(ScaledGlyph {
            rect: cached.rect,
            advance: cached.advance * scale,
            xmin: cached.xmin * scale,
            ymin: cached.ymin * scale,
            width,
            height,
        })
    }

    /// Line packing: copy bitmap into atlas, return atlas rect.
    ///
    /// Shelf packing within `ATLAS_SIZE` is enough (dropped glyphs log warn); a true 2D rect packer like
    /// `etagere` can replace it if dropping ever becomes a problem, no need to add dependency now.
    fn pack(&mut self, bitmap: &[u8], w: usize, h: usize) -> Option<[u32; 4]> {
        let w = w as u32;
        let h = h as u32;
        if self.x + w > ATLAS_SIZE {
            self.x = 0;
            self.y += self.row_h;
            self.row_h = 0;
        }
        if self.y + h > ATLAS_SIZE {
            log::warn!("ui overlay: glyph atlas full ({ATLAS_SIZE}px) — dropping glyph");
            return None;
        }
        let rect = [self.x, self.y, w, h];
        // Copy to host-visible memory.
        let ptr = unsafe {
            self.device
                .map_memory(self.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .expect("GlyphAtlas: map")
        };
        unsafe {
            let base = ptr as *mut u8;
            for row in 0..h {
                let src = &bitmap[(row * w) as usize..((row + 1) * w) as usize];
                let dst = base.add((self.y + row) as usize * self.row_pitch + self.x as usize);
                std::ptr::copy_nonoverlapping(src.as_ptr(), dst, w as usize);
            }
            self.device.unmap_memory(self.memory);
        }
        self.x += w + 1; // 1px gap prevents adjacent glyph bleeding
        self.row_h = self.row_h.max(h);
        self.dirty = true;
        Some(rect)
    }
}

impl Drop for GlyphAtlas {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_image_view(self.view, None);
            self.device.free_memory(self.memory, None);
            self.device.destroy_image(self.image, None);
        }
    }
}

// ---------------------------------------------------------------------------
// UiOverlay
// ---------------------------------------------------------------------------

/// GPU-side UI overlay.
pub struct UiOverlay {
    pipeline: Option<vk::Pipeline>,
    layout: Option<vk::PipelineLayout>,
    set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    /// One set per frame-in-flight: the backdrop view changes between frames,
    /// and writing a set that a still-executing command buffer references is
    /// a validation error, so updates rotate through these instead.
    descriptor_sets: [vk::DescriptorSet; crate::swapchain::FRAMES_IN_FLIGHT],
    /// One host-visible vertex buffer per frame-in-flight so recording frame
    /// N+1 never overwrites vertices still being read by frame N on the GPU.
    vertex_buffers: [vk::Buffer; crate::swapchain::FRAMES_IN_FLIGHT],
    vertex_memories: [vk::DeviceMemory; crate::swapchain::FRAMES_IN_FLIGHT],
    vertex_capacities: [u32; crate::swapchain::FRAMES_IN_FLIGHT],
    render_pass: vk::RenderPass,
    /// Intermediate render pass used when splitting UI draws around a backdrop
    /// capture: leaves the target in `COLOR_ATTACHMENT_OPTIMAL` instead of
    /// `final_layout`. `None` when `final_layout` is already
    /// `COLOR_ATTACHMENT_OPTIMAL`.
    intermediate_render_pass: Option<vk::RenderPass>,
    /// Color format used when creating `render_pass`; used to decide whether
    /// rebuild needed when swapchain format changes (see [`UiOverlay::set_color_format`]).
    color_format: vk::Format,
    /// Image layout at render pass end: swapchain is `PRESENT_SRC_KHR`, offscreen target
    /// has no presentation engine, must stay in `COLOR_ATTACHMENT_OPTIMAL` (#597).
    final_layout: vk::ImageLayout,
    /// Scene-shared bindless texture table layout (set 1, for UI background images, #591).
    /// Owned by [`GraphRenderer`](crate::GraphRenderer), here only borrow handle.
    bindless_layout: vk::DescriptorSetLayout,
    atlas: GlyphAtlas,
    device: ash::Device,
    /// One framebuffer per swapchain image view/extent, reused across frames.
    /// Cleared by [`UiOverlay::on_swapchain_views_changed`] because the old
    /// image views are destroyed during swapchain recreation.
    framebuffers: Vec<CachedFramebuffer>,
    /// Filtered backdrop sampled by surfaces whose material asks for it,
    /// tracked per frame-in-flight descriptor slot so every slot updates when
    /// the view is created or recreated.
    backdrop_views: [vk::ImageView; crate::swapchain::FRAMES_IN_FLIGHT],
    backdrop_sampler: vk::Sampler,
    /// Objects still referenced by in-flight command buffers (see `crate::retire`).
    retire: crate::retire::RetireQueue,
    /// Frame slot being recorded; growth parks the old buffer in this slot.
    frame_index: u32,
}

#[derive(Clone, Copy)]
struct CachedFramebuffer {
    view: vk::ImageView,
    extent: vk::Extent2D,
    handle: vk::Framebuffer,
}

impl UiOverlay {
    /// Construct with default `B8G8R8A8_SRGB` color format.
    ///
    /// Callers knowing actual swapchain format (like [`GraphRenderer`](crate::GraphRenderer))
    /// should use [`UiOverlay::with_color_format`]: when preferred surface format unavailable will
    /// fall back to `available[0]` (commonly `R8G8B8A8_*` on Android), render pass and
    /// image view formats must match (VUID -VkFramebufferCreateInfo-renderPass-02711).
    pub fn new(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        bindless_layout: vk::DescriptorSetLayout,
    ) -> Result<Self> {
        Self::with_color_format(
            context,
            command_pool,
            vk::Format::B8G8R8A8_SRGB,
            bindless_layout,
        )
    }

    /// Construct overlay layer with actual swapchain color format.
    pub fn with_color_format(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        color_format: vk::Format,
        bindless_layout: vk::DescriptorSetLayout,
    ) -> Result<Self> {
        Self::with_color_format_and_final_layout(
            context,
            command_pool,
            color_format,
            vk::ImageLayout::PRESENT_SRC_KHR,
            bindless_layout,
        )
    }

    /// Construct overlay layer with explicit final layout.
    ///
    /// Swapchain uses [`vk::ImageLayout::PRESENT_SRC_KHR`]; offscreen target (editor viewport)
    /// has no presentation engine, must use [`vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL`],
    /// otherwise render pass leaves illegal layout (#597).
    pub fn with_color_format_and_final_layout(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        color_format: vk::Format,
        final_layout: vk::ImageLayout,
        bindless_layout: vk::DescriptorSetLayout,
    ) -> Result<Self> {
        let device = context.device.clone();
        let render_pass = Self::create_render_pass(&device, color_format, final_layout)?;
        let intermediate_render_pass = if final_layout == vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        {
            None
        } else {
            Some(Self::create_render_pass(
                &device,
                color_format,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            )?)
        };
        let atlas = GlyphAtlas::new(context, command_pool)?;

        let init_vertices = 1024u32;
        let buf_size = VERTEX_SIZE * init_vertices as u64;
        let mut vertex_buffers = [vk::Buffer::null(); crate::swapchain::FRAMES_IN_FLIGHT];
        let mut vertex_memories = [vk::DeviceMemory::null(); crate::swapchain::FRAMES_IN_FLIGHT];
        for slot in 0..crate::swapchain::FRAMES_IN_FLIGHT {
            let (buf, mem) = create_buffer(
                context,
                buf_size,
                BufferUsage::VERTEX_BUFFER,
                MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
            )
            .context("UiOverlay: create vertex buffer")?;
            vertex_buffers[slot] = buf;
            vertex_memories[slot] = mem;
        }

        let backdrop_sampler = Self::create_backdrop_sampler(&device)?;

        // Until the renderer supplies a real backdrop, this slot points at the
        // glyph atlas: it is always alive and already in a sampled layout, and
        // the shader only reads the slot for quads that ask for a backdrop.
        let (set_layout, descriptor_pool, descriptor_sets) = Self::create_descriptors(
            &device,
            atlas.view,
            atlas.sampler,
            atlas.view,
            backdrop_sampler,
        )?;

        Ok(Self {
            backdrop_views: [atlas.view; crate::swapchain::FRAMES_IN_FLIGHT],
            backdrop_sampler,
            pipeline: None,
            layout: None,
            set_layout,
            descriptor_pool,
            descriptor_sets,
            vertex_buffers,
            vertex_memories,
            vertex_capacities: [init_vertices; crate::swapchain::FRAMES_IN_FLIGHT],
            render_pass,
            intermediate_render_pass,
            color_format,
            final_layout,
            bindless_layout,
            atlas,
            device,
            framebuffers: Vec::new(),
            retire: crate::retire::RetireQueue::new(),
            frame_index: 0,
        })
    }

    /// Bind the renderer's filtered backdrop for `frame_index`'s descriptor slot.
    ///
    /// Called once per frame (or per backdrop capture) before drawing; each
    /// frame-in-flight slot tracks its own bound view so every slot is updated
    /// on first use and after swapchain resizes.
    pub fn set_backdrop(&mut self, view: vk::ImageView, frame_index: u32) {
        let slot = frame_index as usize % crate::swapchain::FRAMES_IN_FLIGHT;
        if self.backdrop_views[slot] == view {
            return;
        }
        self.backdrop_views[slot] = view;
        self.write_backdrop_descriptor(slot);
    }

    fn create_backdrop_sampler(device: &ash::Device) -> Result<vk::Sampler> {
        unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .min_lod(0.0)
                    .max_lod((crate::ui_backdrop::BACKDROP_MIP_LEVELS - 1) as f32),
                None,
            )
        }
        .context("UiOverlay: backdrop sampler")
    }

    /// Rewrite the backdrop binding of one frame slot's set.
    ///
    /// `slot` selects the set whose command buffer is *not* in flight;
    /// using the slot being recorded keeps the write out of a live submission.
    fn write_backdrop_descriptor(&self, slot: usize) {
        let image_info = vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(self.backdrop_views[slot])
            .sampler(self.backdrop_sampler);
        let set = self.descriptor_sets[slot];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(BACKDROP_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe {
            self.device
                .update_descriptor_sets(std::slice::from_ref(&write), &[]);
        }
    }

    /// Make render pass follow swapchain's actual color format.
    ///
    /// Format may change after swapchain rebuild (fallback to `available[0]` when preferred unavailable,
    /// commonly `R8G8B8A8_*` on Android). On format change rebuild render pass with new format,
    /// and destroy pipelines and framebuffers depending on old pass; pipelines will be
    /// lazily rebuilt on next [`UiOverlay::record`] with new pass.
    pub fn set_color_format(&mut self, color_format: vk::Format) -> Result<()> {
        if self.color_format == color_format {
            return Ok(());
        }
        // Build new first then tear down old, so on create failure old pass still valid.
        let render_pass = Self::create_render_pass(&self.device, color_format, self.final_layout)?;
        let intermediate_render_pass =
            if self.final_layout == vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL {
                None
            } else {
                Some(Self::create_render_pass(
                    &self.device,
                    color_format,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                )?)
            };
        // Pipelines rebuilt on next record with new render pass; here only swap pass. Previously submitted
        // command buffers may still reference this render pass / pipeline, left to recycle queue for delayed destruction.
        {
            if let Some(pipeline) = self.pipeline.take() {
                self.retire.retire(
                    self.frame_index,
                    crate::retire::RetiredResource::Pipeline(pipeline),
                );
            }
            if let Some(layout) = self.layout.take() {
                self.retire.retire(
                    self.frame_index,
                    crate::retire::RetiredResource::PipelineLayout(layout),
                );
            }
            for cached in self.framebuffers.drain(..) {
                self.retire.retire(
                    self.frame_index,
                    crate::retire::RetiredResource::Framebuffer(cached.handle),
                );
            }
            self.retire.retire(
                self.frame_index,
                crate::retire::RetiredResource::RenderPass(self.render_pass),
            );
            if let Some(old_intermediate) = self.intermediate_render_pass.take() {
                self.retire.retire(
                    self.frame_index,
                    crate::retire::RetiredResource::RenderPass(old_intermediate),
                );
            }
        }
        self.render_pass = render_pass;
        self.intermediate_render_pass = intermediate_render_pass;
        self.color_format = color_format;
        log::debug!("UiOverlay: render pass rebuilt for {color_format:?}");
        Ok(())
    }

    /// set 0 binding 0 = glyph atlas combined image sampler.
    fn create_descriptors(
        device: &ash::Device,
        atlas_view: vk::ImageView,
        atlas_sampler: vk::Sampler,
        backdrop_view: vk::ImageView,
        backdrop_sampler: vk::Sampler,
    ) -> Result<(
        vk::DescriptorSetLayout,
        vk::DescriptorPool,
        [vk::DescriptorSet; crate::swapchain::FRAMES_IN_FLIGHT],
    )> {
        let bindings_desc = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(GLYPH_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(BACKDROP_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings_desc);
        let set_layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .context("UiOverlay: descriptor set layout")?;

        // Two bindings per set, one set per frame-in-flight: the pool has to
        // hold every descriptor those sets will ever use, not just one set's.
        let pool_size = vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 2 * crate::swapchain::FRAMES_IN_FLIGHT as u32,
        };
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(crate::swapchain::FRAMES_IN_FLIGHT as u32)
            .pool_sizes(std::slice::from_ref(&pool_size));
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .context("UiOverlay: descriptor pool")?;

        let layouts = [set_layout; crate::swapchain::FRAMES_IN_FLIGHT];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        let allocated = unsafe { device.allocate_descriptor_sets(&alloc_info) }
            .context("UiOverlay: allocate descriptor sets")?;

        let atlas_info = vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::GENERAL)
            .image_view(atlas_view)
            .sampler(atlas_sampler);
        let backdrop_info = vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::GENERAL)
            .image_view(backdrop_view)
            .sampler(backdrop_sampler);
        for set in &allocated {
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(GLYPH_ATLAS_BINDING)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(std::slice::from_ref(&atlas_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(BACKDROP_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(std::slice::from_ref(&backdrop_info)),
            ];
            unsafe {
                device.update_descriptor_sets(&writes, &[]);
            }
        }

        let mut sets = [vk::DescriptorSet::null(); crate::swapchain::FRAMES_IN_FLIGHT];
        sets.copy_from_slice(&allocated);
        Ok((set_layout, pool, sets))
    }

    fn create_render_pass(
        device: &ash::Device,
        color_format: vk::Format,
        final_layout: vk::ImageLayout,
    ) -> Result<vk::RenderPass> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(final_layout);

        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));

        let dep = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_READ,
            );

        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&color_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dep));

        let rp = unsafe { device.create_render_pass(&create_info, None) }
            .context("UiOverlay: create_render_pass")?;
        Ok(rp)
    }

    fn ensure_pipeline(&mut self, device: &ash::Device, extent: vk::Extent2D) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let (pipeline, layout) =
            Self::create_pipeline(device, self.render_pass, extent, self.bindless_layout)?;
        self.pipeline = Some(pipeline);
        self.layout = Some(layout);
        Ok(())
    }

    fn create_pipeline(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        _extent: vk::Extent2D,
        bindless_layout: vk::DescriptorSetLayout,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/ui_overlay.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/ui_overlay.frag.spv");
        let vert_module =
            crate::shader::load_shader_module(device, VERT_SPV).context("UiOverlay: load vert")?;
        let frag_module =
            crate::shader::load_shader_module(device, FRAG_SPV).context("UiOverlay: load frag")?;

        let vert_entry = CString::new(ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = CString::new(ENTRY_FRAGMENT_MAIN).unwrap();
        let shader_stages = [
            crate::shader::shader_stage(
                vk::ShaderStageFlags::VERTEX,
                vert_module,
                vert_entry.as_c_str(),
            ),
            crate::shader::shader_stage(
                vk::ShaderStageFlags::FRAGMENT,
                frag_module,
                frag_entry.as_c_str(),
            ),
        ];

        // set 0 binding 0 — glyph atlas (see GlyphAtlas::create_descriptors).
        let bindings_desc = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(GLYPH_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(BACKDROP_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings_desc);
        let set_layout = unsafe { device.create_descriptor_set_layout(&set_layout_info, None) }
            .context("UiOverlay: pipeline set layout")?;
        // set 1 = scene-shared bindless texture table (UI background images, #591).
        let set_layouts = [set_layout, bindless_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .context("UiOverlay: pipeline layout")?;
        unsafe {
            device.destroy_descriptor_set_layout(set_layout, None);
        }

        let binding_desc = vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(size_of::<UiVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(2)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(16),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(3)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(32),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(4)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(40),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(5)
                .format(vk::Format::R32_SFLOAT)
                .offset(48),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(6)
                .format(vk::Format::R32_UINT)
                .offset(52),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(7)
                .format(vk::Format::R32_SFLOAT)
                .offset(56),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(8)
                .format(vk::Format::R8G8B8A8_UNORM)
                .offset(60),
        ];
        let vertex_input_info = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(std::slice::from_ref(&binding_desc))
            .vertex_attribute_descriptions(&attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state_info =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        // Pipeline state: no depth, alpha blend.
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD);
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .logic_op_enable(false)
            .attachments(std::slice::from_ref(&blend));
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input_info)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state_info)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipeline = unsafe {
            device
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    std::slice::from_ref(&pipeline_info),
                    None,
                )
                .map_err(|(_, e)| e)
        }
        .context("UiOverlay: graphics pipeline")?[0];

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }
        Ok((pipeline, layout))
    }

    fn grow_buffer(&mut self, context: &VulkanContext, slot: usize, needed: u32) -> Result<()> {
        if needed <= self.vertex_capacities[slot] {
            return Ok(());
        }
        let new_cap = needed.next_power_of_two();
        let buf_size = VERTEX_SIZE * new_cap as u64;
        let (buf, mem) = create_buffer(
            context,
            buf_size,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("UiOverlay: grow vertex buffer")?;
        // The previous frames' command buffers may still read the old buffer, and
        // growth happens mid-frame while the current one is recording. Park it in
        // the retirement queue instead of stalling the device.
        if self.vertex_buffers[slot] != vk::Buffer::null()
            || self.vertex_memories[slot] != vk::DeviceMemory::null()
        {
            self.retire.retire(
                self.frame_index,
                crate::retire::RetiredResource::Buffer {
                    buffer: self.vertex_buffers[slot],
                    memory: self.vertex_memories[slot],
                },
            );
        }
        self.vertex_buffers[slot] = buf;
        self.vertex_memories[slot] = mem;
        self.vertex_capacities[slot] = new_cap;
        Ok(())
    }

    /// Estimate vertex count: quads * 6 + chars * 6 (doubled when drop shadow is enabled).
    fn estimate_vertex_count(&self, input: &UiOverlayInput) -> u32 {
        let mut chars = 0usize;
        for t in &input.texts {
            let mult = if t.shadow_color[3] > 0.0 { 2 } else { 1 };
            chars += t.content.chars().count() * mult;
        }
        input.quads.len() as u32 * VERTICES_PER_QUAD + (chars as u32) * VERTICES_PER_QUAD
    }

    /// Record UI overlay draw commands into `cmd`.
    #[expect(
        clippy::too_many_arguments,
        reason = "recording needs the command buffer plus the per-frame UI inputs"
    )]
    pub fn record(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        // Target rect (#988): when `internal_resolution` active UI only draws inside this block,
        // composited with PostPass-upscaled image, not pressed onto letterbox.
        dest: vk::Rect2D,
        target_view: vk::ImageView,
        input: &UiOverlayInput,
        // Scene bindless texture table (set 1); UI background images sampled by `UiQuad::texture`.
        bindless_set: vk::DescriptorSet,
        frame_index: u32,
    ) -> Result<()> {
        self.record_with_backdrop(
            context,
            cmd,
            extent,
            dest,
            target_view,
            input,
            bindless_set,
            frame_index,
            None,
        )
    }

    /// Record UI overlay draw commands into `cmd`, splitting render passes
    /// around backdrop-sampling quads so `on_backdrop` can capture and blur
    /// the already-drawn underlying UI and scene into `backdrop_views`.
    #[expect(
        clippy::too_many_arguments,
        reason = "recording needs the command buffer, per-frame UI inputs, and optional backdrop callback"
    )]
    pub fn record_with_backdrop(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        dest: vk::Rect2D,
        target_view: vk::ImageView,
        input: &UiOverlayInput,
        bindless_set: vk::DescriptorSet,
        frame_index: u32,
        mut on_backdrop: Option<&mut dyn FnMut(vk::CommandBuffer, f32) -> Result<vk::ImageView>>,
    ) -> Result<()> {
        // The frame slot that just waited its fence is free again: release what
        // this overlay parked during the previous pass through the queue.
        self.retire.collect(&context.device, frame_index);
        self.frame_index = frame_index;
        if input.is_empty() {
            return Ok(());
        }
        let pipeline_start = std::time::Instant::now();
        self.ensure_pipeline(&context.device, extent)?;
        log_slow_step("ensure_pipeline", pipeline_start.elapsed());

        // The estimate only sizes the reservation: a character writes vertices
        // only when it has a glyph bitmap (spaces have no rect, and a full atlas
        // or missing font skips entirely), so the real count is usually lower.
        let estimated_vertices = self.estimate_vertex_count(input);

        // Build vertices (quads + per-glyph expanded texts).
        let mut vertices: Vec<UiVertex> = Vec::with_capacity(estimated_vertices as usize);
        let w = dest.extent.width as f32;
        let h = dest.extent.height as f32;

        // Interleave rects and text by engine-computed layer / order (old path without order degrades to
        // "all rects first, then all text").
        let push_quad = |vertices: &mut Vec<UiVertex>, quad: &crate::ui_overlay::UiQuad| {
            let [x0, y0, x1, y1] = quad.rect;
            let [r, g, b, a] = quad.color;
            // NDC rect → pixel size. `UiQuad::border_radius` is already
            // normalised by the engine (`radius_px / max(w, h) * 2`), so
            // convert it back to pixels for the SDF.
            let width_px = (x1 - x0).abs() * 0.5 * w;
            let height_px = (y1 - y0).abs() * 0.5 * h;
            let half = [width_px * 0.5, height_px * 0.5];
            let radius = (quad.border_radius * width_px.max(height_px) * 0.5)
                .clamp(0.0, half[0].min(half[1]));
            // With background image uv covers 0..1; solid quad samples atlas 1×1 white pixel.
            let (uv0, uv1) = if quad.texture.is_some() {
                ([0.0, 0.0], [1.0, 1.0])
            } else {
                ([0.0, 0.0], [0.0, 0.0])
            };
            emit_quad(
                vertices,
                [x0, y0],
                [x1, y1],
                uv0,
                uv1,
                [r, g, b, a],
                Some((half, radius)),
                quad.texture.unwrap_or(u32::MAX),
                quad.backdrop_sigma,
            );
        };

        let mut tracker = BackdropSegmentTracker::new(on_backdrop.is_some(), w, h);

        if input.order.is_empty() {
            for quad in &input.quads {
                tracker.on_quad(quad, vertices.len());
                push_quad(&mut vertices, quad);
            }
            for t in &input.texts {
                tracker.on_text(text_ndc_rect(t, w, h));
                self.emit_text(&mut vertices, t, w, h);
            }
        } else {
            for draw in &input.order {
                match *draw {
                    crate::ui_overlay::UiOverlayDraw::Quad(index) => {
                        if let Some(quad) = input.quads.get(index as usize) {
                            tracker.on_quad(quad, vertices.len());
                            push_quad(&mut vertices, quad);
                        }
                    }
                    crate::ui_overlay::UiOverlayDraw::Text(index) => {
                        if let Some(t) = input.texts.get(index as usize) {
                            tracker.on_text(text_ndc_rect(t, w, h));
                            self.emit_text(&mut vertices, t, w, h);
                        }
                    }
                }
            }
        }
        let segments = tracker.finish(vertices.len());

        // Upload and draw both use actual vertex count: estimate may exceed `vertices.len()`,
        // copying that much would read uninitialized memory past Vec (UB, stray triangles on GPU).
        let vert_count =
            u32::try_from(vertices.len()).context("UiOverlay: vertex count exceeds u32")?;
        if vert_count == 0 {
            return Ok(());
        }
        let slot = frame_index as usize % crate::swapchain::FRAMES_IN_FLIGHT;
        let buffer_start = std::time::Instant::now();
        self.grow_buffer(context, slot, vert_count)?;
        log_slow_step("grow_buffer", buffer_start.elapsed());

        // Upload vertex data into this frame-in-flight slot's buffer.
        let vert_bytes = vertices.len() * size_of::<UiVertex>();
        unsafe {
            let ptr = context
                .device
                .map_memory(
                    self.vertex_memories[slot],
                    0,
                    vert_bytes as u64,
                    vk::MemoryMapFlags::empty(),
                )
                .context("UiOverlay: map")?;
            std::ptr::copy_nonoverlapping(
                bytemuck::cast_slice(&vertices).as_ptr(),
                ptr as *mut u8,
                vert_bytes,
            );
            context.device.unmap_memory(self.vertex_memories[slot]);
        }

        // Atlas has new glyphs → HOST_WRITE → FRAGMENT_SHADER barrier (GENERAL layout unchanged).
        if self.atlas.dirty {
            let barrier = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.atlas.image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_access_mask(vk::AccessFlags::HOST_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ);
            unsafe {
                context.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    std::slice::from_ref(&barrier),
                );
            }
            self.atlas.dirty = false;
        }

        // Reuse the framebuffer for this swapchain image instead of creating
        // (and retaining) one every frame. Not thread-safe by itself; record
        // runs on the render thread and swapchain recreation clears the cache.
        let fb = match self.framebuffers.iter().find(|cached| {
            cached.view == target_view
                && cached.extent.width == extent.width
                && cached.extent.height == extent.height
        }) {
            Some(cached) => cached.handle,
            None => {
                let attachments = [target_view];
                let fb_info = vk::FramebufferCreateInfo::default()
                    .render_pass(self.render_pass)
                    .attachments(&attachments)
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1);
                let handle = unsafe { context.device.create_framebuffer(&fb_info, None) }
                    .context("UiOverlay: framebuffer")?;
                self.framebuffers.push(CachedFramebuffer {
                    view: target_view,
                    extent,
                    handle,
                });
                handle
            }
        };

        // Record draw segments, capturing and blurring the backdrop before any
        // segment that introduces frosted-glass surfaces.
        let last_idx = segments.len().saturating_sub(1);
        for (i, seg) in segments.iter().enumerate() {
            if let (Some(sigma), Some(capture_fn)) =
                (seg.capture_before_sigma, on_backdrop.as_deref_mut())
            {
                if let Err(err) = capture_fn(cmd, sigma) {
                    log::warn!("ui backdrop: filter pass failed, drawing flat surfaces: {err}");
                }
            }

            let rp = if i == last_idx {
                self.render_pass
            } else {
                self.intermediate_render_pass.unwrap_or(self.render_pass)
            };
            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(rp)
                .framebuffer(fb)
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent,
                });
            unsafe {
                context
                    .device
                    .cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
                context.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline.unwrap(),
                );
                context.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout.unwrap(),
                    0,
                    std::slice::from_ref(
                        &self.descriptor_sets
                            [frame_index as usize % crate::swapchain::FRAMES_IN_FLIGHT],
                    ),
                    &[],
                );
                if bindless_set != vk::DescriptorSet::null() {
                    context.device.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.layout.unwrap(),
                        1,
                        std::slice::from_ref(&bindless_set),
                        &[],
                    );
                }

                // Map NDC [-1,1] to `dest`: UI and scene scale together and stay centered
                // inside `dest`, while scissor clips any out-of-bounds pixels.
                let vp = vk::Viewport::default()
                    .x(dest.offset.x as f32)
                    .y(dest.offset.y as f32)
                    .width(dest.extent.width as f32)
                    .height(dest.extent.height as f32)
                    .min_depth(0.0)
                    .max_depth(1.0);
                let sc = dest;
                context
                    .device
                    .cmd_set_viewport(cmd, 0, std::slice::from_ref(&vp));
                context
                    .device
                    .cmd_set_scissor(cmd, 0, std::slice::from_ref(&sc));

                let bufs = [self.vertex_buffers[slot]];
                context
                    .device
                    .cmd_bind_vertex_buffers(cmd, 0, &bufs, &[0u64]);
                context
                    .device
                    .cmd_draw(cmd, seg.vertex_count, 1, seg.first_vertex, 0);
                context.device.cmd_end_render_pass(cmd);
            }
        }
        Ok(())
    }

    /// Discard framebuffers built for the previous swapchain image views.
    ///
    /// Must be called after the device is idle and **before** the old
    /// swapchain image views are destroyed, otherwise the cached framebuffers
    /// keep references to destroyed views (VUID-vkDestroyImageView-...).
    pub fn on_swapchain_views_changed(&mut self) {
        for cached in self.framebuffers.drain(..) {
            unsafe { self.device.destroy_framebuffer(cached.handle, None) };
        }
    }

    /// Register a project-bundled font (raw bytes of `assets/fonts/*.ttf|otf`, #583).
    ///
    /// Name is exactly the asset path written in `Text.font`; parse failure returns `false`, text falls back to
    /// system default font. Callable anytime (atlas is host-visible, new glyphs packed only on
    /// next draw).
    pub fn register_font(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        self.atlas.register_font(name, bytes)
    }

    /// Expand one text command into per-glyph quads (pixels → NDC, glyphs carry atlas uv).
    fn emit_text(&mut self, vertices: &mut Vec<UiVertex>, t: &UiTextInput, w: f32, h: f32) {
        let [left, top, rect_w, rect_h] = t.rect;
        if t.font_size <= 0.0 || rect_w <= 0.0 || rect_h <= 0.0 {
            return;
        }
        if self.atlas.rasterizer.is_none() {
            return;
        }

        let font = t.font.as_deref();
        let use_sdf = (t.outline_width > 0.0 && t.outline_color[3] > 0.0) || t.font_size > 24.0;

        // 1. Line breaking (#527): `\n` always breaks; with `wrap` also greedy break by rect width.
        //    Without `wrap` nor `\n` it's original single line, behavior unchanged.
        let lines = if t.wrap {
            let mut advance = |ch: char| self.advance_of(font, t.font_size, ch, use_sdf);
            wrap_text(&t.content, rect_w, &mut advance)
        } else {
            t.content.split('\n').map(str::to_string).collect()
        };
        if lines.is_empty() {
            return;
        }

        let line_height = self.atlas.line_height(font, t.font_size);
        let [r, g, b, a] = t.color;

        for (index, line) in lines.iter().enumerate() {
            let line_top = top + index as f32 * line_height;
            // Lines beyond rect bottom not drawn (no half-glyph clipping).
            if line_top > top + rect_h {
                break;
            }

            // 2. Measure this line's width (sum of advances).
            let mut total = 0.0f32;
            for ch in line.chars() {
                total += self.advance_of(font, t.font_size, ch, use_sdf);
            }
            if total <= 0.0 {
                continue;
            }

            // 3. Alignment decides starting x (per-line alignment, not whole paragraph).
            let start_x = match t.alignment {
                UiTextAlign::Left => left,
                UiTextAlign::Center => left + (rect_w - total) * 0.5,
                UiTextAlign::Right => left + rect_w - total,
            };

            // 4. Emit per-glyph quads (shadow pass first, then main face + optional SDF outline).
            let baseline = line_top + self.atlas.baseline_offset(font, t.font_size);
            let scale = t.font_size / SDF_FONT_SIZE;
            let outline_norm = if use_sdf && t.outline_width > 0.0 && t.outline_color[3] > 0.0 {
                (0.5 * t.outline_width / (SDF_SPREAD * scale.max(0.05))).clamp(0.0, 0.42)
            } else {
                0.0
            };
            let base_radius = if use_sdf { -2.0 } else { -1.0 };
            let main_radius = if use_sdf { -2.0 - outline_norm } else { -1.0 };
            let outline_rgba8 = pack_linear_unorm(srgb_rgba_to_linear(t.outline_color));
            let has_shadow = t.shadow_color[3] > 0.0;

            let mut emit_line_glyphs =
                |offset: [f32; 2], color: [f32; 4], radius_val: f32, outline: [u8; 4]| {
                    let mut cursor_x = start_x + offset[0];
                    for ch in line.chars() {
                        let Some(glyph) = self.atlas.glyph(font, ch, t.font_size, use_sdf) else {
                            continue;
                        };
                        if let Some([gx, gy, gw, gh]) = glyph.rect {
                            let px = cursor_x + glyph.xmin;
                            let py = baseline + offset[1] - glyph.ymin - glyph.height;
                            let x0 = (px / w) * 2.0 - 1.0;
                            let y0 = pixel_y_to_ndc(py, h);
                            let x1 = ((px + glyph.width) / w) * 2.0 - 1.0;
                            let y1 = pixel_y_to_ndc(py + glyph.height, h);
                            let inv = 1.0 / ATLAS_SIZE as f32;
                            let u0 = gx as f32 * inv;
                            let v0 = gy as f32 * inv;
                            let u1 = (gx + gw) as f32 * inv;
                            let v1 = (gy + gh) as f32 * inv;
                            emit_quad_raw(
                                vertices,
                                [x0, y0],
                                [x1, y1],
                                [u0, v0],
                                [u1, v1],
                                color,
                                [0.0; 2],
                                [0.0; 2],
                                radius_val,
                                u32::MAX,
                                0.0,
                                outline,
                            );
                        }
                        cursor_x += glyph.advance;
                    }
                };

            if has_shadow {
                emit_line_glyphs(t.shadow_offset, t.shadow_color, base_radius, [0; 4]);
            }
            emit_line_glyphs([0.0, 0.0], [r, g, b, a], main_radius, outline_rgba8);
        }
    }

    /// Advance of single char (px); 0 if font lacks glyph.
    fn advance_of(&mut self, font: Option<&str>, font_size: f32, ch: char, use_sdf: bool) -> f32 {
        self.atlas
            .glyph(font, ch, font_size, use_sdf)
            .map(|glyph| glyph.advance)
            .filter(|advance| advance.is_finite() && *advance > 0.0)
            .unwrap_or(0.0)
    }
}

/// Max lines when `wrap` (guard against huge strings).
const MAX_WRAP_LINES: usize = 4096;

/// Greedy line breaking by width (#527).
///
/// - `\n` always produces new line (empty lines preserved, letting author control paragraph spacing);
/// - Break points from UAX #14 (`unicode-linebreak`): spaces, hyphens and between CJK chars can break,
///   CJK trailing punctuation such as comma / full stop won't be pushed to line start;
/// - Single word wider than whole line broken by char;
/// - Trailing spaces removed after break, leading spaces likewise.
///
/// Width uses font advance (pixels), so `textwrap` not used: it lays out by
/// `unicode-width` column widths, which don't match actual font pixel widths.
fn wrap_text(content: &str, max_width: f32, advance: &mut impl FnMut(char) -> f32) -> Vec<String> {
    let max_width = if max_width.is_finite() && max_width > 0.0 {
        max_width
    } else {
        f32::MAX
    };
    let mut lines: Vec<String> = Vec::new();
    for paragraph in content.split('\n') {
        wrap_paragraph(paragraph, max_width, advance, &mut lines);
        if lines.len() >= MAX_WRAP_LINES {
            lines.truncate(MAX_WRAP_LINES);
            break;
        }
    }
    lines
}

fn wrap_paragraph(
    paragraph: &str,
    max_width: f32,
    advance: &mut impl FnMut(char) -> f32,
    lines: &mut Vec<String>,
) {
    if paragraph.is_empty() {
        lines.push(String::new());
        return;
    }

    // Byte offset → char index, and cumulative advance per char: line width for any `[from, to)` is
    // O(1), no need to re-measure for every candidate break point.
    let mut char_offsets: Vec<usize> = paragraph.char_indices().map(|(offset, _)| offset).collect();
    char_offsets.push(paragraph.len());
    let mut prefix = Vec::with_capacity(char_offsets.len());
    prefix.push(0.0f32);
    for ch in paragraph.chars() {
        let next = prefix[prefix.len() - 1] + advance(ch).max(0.0);
        prefix.push(next);
    }
    let char_index = |byte: usize| char_offsets.binary_search(&byte).unwrap_or_else(|at| at);
    let width = |from: usize, to: usize| prefix[char_index(to)] - prefix[char_index(from)];

    let breaks: Vec<usize> = unicode_linebreak::linebreaks(paragraph)
        .map(|(offset, _)| offset)
        .collect();
    let mut pos = 0usize;
    loop {
        while paragraph[pos..].starts_with(' ') {
            pos += 1;
        }

        // Farthest break point from `pos` such that whole line width still fits.
        let mut last_fit: Option<usize> = None;
        for &brk in &breaks[breaks.partition_point(|&brk| brk <= pos)..] {
            if width(pos, brk) > max_width {
                break;
            }
            last_fit = Some(brk);
        }

        if let Some(brk) = last_fit {
            lines.push(paragraph[pos..brk].trim_end().to_string());
            pos = brk;
            if pos >= paragraph.len() {
                return;
            }
        } else {
            // No break point fits: consume at least one char, truncate by char.
            let mut end = pos;
            for ch in paragraph[pos..].chars() {
                let next = end + ch.len_utf8();
                if end > pos && width(pos, next) > max_width {
                    break;
                }
                end = next;
            }
            lines.push(paragraph[pos..end].to_string());
            pos = end;
            if pos >= paragraph.len() {
                return;
            }
        }

        if lines.len() >= MAX_WRAP_LINES {
            return;
        }
    }
}

fn pack_linear_unorm(c: [f32; 4]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

#[expect(
    clippy::too_many_arguments,
    reason = "low-level vertex emitter for quads and SDF glyphs"
)]
fn emit_quad_raw(
    out: &mut Vec<UiVertex>,
    p0: [f32; 2],
    p1: [f32; 2],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
    local_override: [f32; 2],
    half_override: [f32; 2],
    radius_override: f32,
    texture: u32,
    backdrop: f32,
    outline_color: [u8; 4],
) {
    let [x0, y0] = p0;
    let [x1, y1] = p1;
    let [u0, v0] = uv0;
    let [u1, v1] = uv1;
    let color = srgb_rgba_to_linear(color);
    let use_half = radius_override >= 0.0;
    for &(px, py, u, v, sx, sy) in &[
        (x0, y0, u0, v0, -1.0, -1.0),
        (x1, y0, u1, v0, 1.0, -1.0),
        (x0, y1, u0, v1, -1.0, 1.0),
        (x1, y1, u1, v1, 1.0, 1.0),
        (x1, y0, u1, v0, 1.0, -1.0),
        (x0, y1, u0, v1, -1.0, 1.0),
    ] {
        let local = if use_half {
            [sx * half_override[0], sy * half_override[1]]
        } else {
            local_override
        };
        out.push(UiVertex {
            pos: [px, py],
            uv: [u, v],
            color,
            local,
            half_size: half_override,
            radius: radius_override,
            texture,
            backdrop,
            outline_color,
        });
    }
}

/// Push one quad (two clockwise triangles) into the vertex stream.
#[expect(
    clippy::too_many_arguments,
    reason = "a quad is four corners plus its UV rect and colour"
)]
fn emit_quad(
    out: &mut Vec<UiVertex>,
    p0: [f32; 2],
    p1: [f32; 2],
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
    rounded: Option<([f32; 2], f32)>,
    texture: u32,
    backdrop: f32,
) {
    let (half, radius) = rounded.unwrap_or(([0.0; 2], -1.0));
    emit_quad_raw(
        out, p0, p1, uv0, uv1, color, [0.0; 2], half, radius, texture, backdrop, [0; 4],
    );
}

impl Drop for UiOverlay {
    fn drop(&mut self) {
        // `GraphRenderer::destroy` waits for the device before dropping overlays.
        self.retire.destroy_all(&self.device);
        unsafe {
            if let Some(p) = self.pipeline {
                self.device.destroy_pipeline(p, None);
            }
            if let Some(l) = self.layout {
                self.device.destroy_pipeline_layout(l, None);
            }
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            for cached in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(cached.handle, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
            if let Some(rp) = self.intermediate_render_pass {
                self.device.destroy_render_pass(rp, None);
            }
            for slot in 0..crate::swapchain::FRAMES_IN_FLIGHT {
                self.device.destroy_buffer(self.vertex_buffers[slot], None);
                self.device.free_memory(self.vertex_memories[slot], None);
            }
            self.device.destroy_sampler(self.backdrop_sampler, None);
        }
    }
}

// ---------------------------------------------------------------------------
// One-off command buffer / memory type helpers (same mode as ibl.rs)
// ---------------------------------------------------------------------------

fn allocate_temp_command_buffer(
    device: &ash::Device,
    pool: vk::CommandPool,
) -> anyhow::Result<vk::CommandBuffer> {
    let cmd = unsafe {
        device.allocate_command_buffers(&vk::CommandBufferAllocateInfo {
            command_pool: pool,
            level: vk::CommandBufferLevel::PRIMARY,
            command_buffer_count: 1,
            ..Default::default()
        })
    }?[0];
    Ok(cmd)
}

fn submit_and_wait(
    device: &ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
) -> anyhow::Result<()> {
    let cmd_arr = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmd_arr);
    let fence = unsafe {
        device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .context("UiOverlay submit_and_wait: create fence")?
    };
    unsafe {
        device
            .queue_submit(queue, &[submit], fence)
            .context("UiOverlay submit_and_wait: queue_submit")?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .context("UiOverlay submit_and_wait: wait_for_fences")?;
        device.destroy_fence(fence, None);
        device.free_command_buffers(pool, &cmd_arr);
    }
    Ok(())
}

fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
) -> u32 {
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0
            && props.memory_types[i as usize]
                .property_flags
                .contains(flags)
        {
            return i;
        }
    }
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0 {
            return i;
        }
    }
    panic!("no suitable memory type found");
}

/// sRGB component → linear. UI's `[1, 0.502, 0.125]` should appear as `#FF8020` right out of image,
/// not re-encoded by swapchain into a brighter color (#592).
fn srgb_channel_to_linear(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_rgba_to_linear(color: [f32; 4]) -> [f32; 4] {
    [
        srgb_channel_to_linear(color[0]),
        srgb_channel_to_linear(color[1]),
        srgb_channel_to_linear(color[2]),
        color[3].clamp(0.0, 1.0),
    ]
}

/// Pixel y (origin top-left, growing downward) → Vulkan NDC y (`-1` = screen top, `+1` = bottom).
///
/// Camera projection's `p[1][1] = -inv_tan` is flipped for this convention, so UI here **must not flip again**:
/// earlier writing `(h - py) / h * 2 - 1` (OpenGL Y-up intuition) drew whole UI layer upside-down mirror
/// — intro title displayed inverted, bottom-anchored prompt moved to screen top.
/// Convert an 8-bit antialiased glyph bitmap (`w × h`) into a padded Signed
/// Distance Field (`(w + 2*pad) × (h + 2*pad)`), where `128` (`0.5`) marks the
/// contour and `spread` pixels map across `[0.0, 1.0]`.
fn bitmap_to_sdf(
    bitmap: &[u8],
    w: usize,
    h: usize,
    pad: usize,
    spread: f32,
) -> (Vec<u8>, usize, usize) {
    let sw = w + pad * 2;
    let sh = h + pad * 2;
    let n = sw * sh;
    const INF: f32 = 1e12;

    let mut outside = vec![INF; n];
    let mut inside = vec![0.0f32; n];
    for y in 0..h {
        for x in 0..w {
            let a = bitmap[y * w + x];
            let idx = (y + pad) * sw + (x + pad);
            if a >= 128 {
                outside[idx] = 0.0;
                inside[idx] = INF;
            }
        }
    }

    edt_2d(&mut outside, sw, sh);
    edt_2d(&mut inside, sw, sh);

    let inv_spread = 0.5 / spread.max(1e-3);
    let mut sdf = Vec::with_capacity(n);
    for i in 0..n {
        // Subtract 0.5 so the 0-contour sits midway between adjacent inside/outside
        // pixel centers with exact unit slope across the boundary.
        let dist = (inside[i].sqrt() - 0.5).max(0.0) - (outside[i].sqrt() - 0.5).max(0.0);
        let norm = (0.5 + dist * inv_spread).clamp(0.0, 1.0);
        sdf.push((norm * 255.0).round() as u8);
    }
    (sdf, sw, sh)
}

/// 2D Euclidean squared-distance transform (`O(w * h)`) via separable 1D passes.
fn edt_2d(grid: &mut [f32], w: usize, h: usize) {
    let max_dim = w.max(h);
    let mut f = vec![0.0f32; max_dim];
    let mut v = vec![0usize; max_dim];
    let mut z = vec![0.0f32; max_dim + 1];

    // Vertical pass (columns)
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], &mut v, &mut z, |y, val| grid[y * w + x] = val);
    }
    // Horizontal pass (rows)
    for y in 0..h {
        let row = &grid[y * w..(y + 1) * w];
        f[..w].copy_from_slice(row);
        edt_1d(&f[..w], &mut v, &mut z, |x, val| grid[y * w + x] = val);
    }
}

fn edt_1d(f: &[f32], v: &mut [usize], z: &mut [f32], mut write: impl FnMut(usize, f32)) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;

    for q in 1..n {
        let fq = f[q];
        loop {
            let r = v[k];
            let s =
                ((fq + (q * q) as f32) - (f[r] + (r * r) as f32)) / (2.0 * (q as f32 - r as f32));
            if s <= z[k] {
                if k == 0 {
                    break;
                }
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
    }

    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let r = v[k];
        let dx = q as f32 - r as f32;
        write(q, dx * dx + f[r]);
    }
}

fn pixel_y_to_ndc(py: f32, h: f32) -> f32 {
    (py / h) * 2.0 - 1.0
}

#[derive(Debug, PartialEq)]
struct DrawSegment {
    first_vertex: u32,
    vertex_count: u32,
    capture_before_sigma: Option<f32>,
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

fn expand_ndc(rect: [f32; 4], sigma: f32, w: f32, h: f32) -> [f32; 4] {
    let px = (sigma * 3.0) / w.max(1.0) * 2.0;
    let py = (sigma * 3.0) / h.max(1.0) * 2.0;
    [rect[0] - px, rect[1] - py, rect[2] + px, rect[3] + py]
}

fn text_ndc_rect(t: &crate::ui_overlay::UiTextInput, w: f32, h: f32) -> [f32; 4] {
    let [px, py, pw, ph] = t.rect;
    let x0 = (px / w.max(1.0)) * 2.0 - 1.0;
    let y0 = (py / h.max(1.0)) * 2.0 - 1.0;
    let x1 = ((px + pw) / w.max(1.0)) * 2.0 - 1.0;
    let y1 = ((py + ph) / h.max(1.0)) * 2.0 - 1.0;
    [x0, y0, x1, y1]
}

/// Splits UI draw commands into render-pass segments, only triggering a new
/// backdrop capture when a frosted-glass quad overlaps content drawn since the
/// previous capture.
struct BackdropSegmentTracker {
    segments: Vec<DrawSegment>,
    seg_start: u32,
    seg_capture: Option<f32>,
    has_captured: bool,
    dirty_rects: Vec<[f32; 4]>,
    pending_glass_rects: Vec<[f32; 4]>,
    enabled: bool,
    viewport_w: f32,
    viewport_h: f32,
}

impl BackdropSegmentTracker {
    fn new(enabled: bool, viewport_w: f32, viewport_h: f32) -> Self {
        Self {
            segments: Vec::new(),
            seg_start: 0,
            seg_capture: None,
            has_captured: false,
            dirty_rects: Vec::new(),
            pending_glass_rects: Vec::new(),
            enabled,
            viewport_w,
            viewport_h,
        }
    }

    fn close_segment(&mut self, cur_len: usize) {
        let cur = cur_len as u32;
        if cur > self.seg_start {
            self.segments.push(DrawSegment {
                first_vertex: self.seg_start,
                vertex_count: cur - self.seg_start,
                capture_before_sigma: self.seg_capture.take(),
            });
            self.seg_start = cur;
        }
    }

    fn on_quad(&mut self, quad: &crate::ui_overlay::UiQuad, cur_len: usize) {
        if self.enabled && quad.backdrop_sigma > 0.0 {
            let check_rect = expand_ndc(
                quad.rect,
                quad.backdrop_sigma,
                self.viewport_w,
                self.viewport_h,
            );
            let overlaps_dirty = self
                .dirty_rects
                .iter()
                .any(|r| rects_overlap(*r, check_rect));
            if !self.has_captured || overlaps_dirty {
                self.close_segment(cur_len);
                self.seg_capture = Some(quad.backdrop_sigma);
                self.has_captured = true;
                self.dirty_rects.clear();
                self.pending_glass_rects.clear();
            } else if let Some(s) = self.seg_capture.as_mut() {
                *s = s.max(quad.backdrop_sigma);
            }
            self.pending_glass_rects.push(quad.rect);
        } else {
            if !self.pending_glass_rects.is_empty() {
                self.dirty_rects.append(&mut self.pending_glass_rects);
            }
            self.dirty_rects.push(quad.rect);
        }
    }

    fn on_text(&mut self, ndc_rect: [f32; 4]) {
        if !self.pending_glass_rects.is_empty() {
            self.dirty_rects.append(&mut self.pending_glass_rects);
        }
        self.dirty_rects.push(ndc_rect);
    }

    fn finish(mut self, total_vertices: usize) -> Vec<DrawSegment> {
        self.close_segment(total_vertices);
        self.segments
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bitmap_to_sdf, pixel_y_to_ndc, srgb_rgba_to_linear, wrap_text, BackdropSegmentTracker,
    };

    /// Monospace 10px metrics, line-break tests only care about width, not real font.
    fn ten_px(_ch: char) -> f32 {
        10.0
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_respects_the_width() {
        let mut advance = ten_px;
        let lines = wrap_text("aa bb cc", 60.0, &mut advance);
        assert_eq!(lines, vec!["aa bb".to_string(), "cc".to_string()]);
    }

    #[test]
    fn a_word_longer_than_the_line_breaks_by_character() {
        let mut advance = ten_px;
        let lines = wrap_text("abcdef", 30.0, &mut advance);
        assert_eq!(lines, vec!["abc".to_string(), "def".to_string()]);
    }

    #[test]
    fn newlines_always_break_and_keep_blank_lines() {
        let mut advance = ten_px;
        let lines = wrap_text("a\n\nb", 100.0, &mut advance);
        assert_eq!(lines, vec!["a".to_string(), String::new(), "b".to_string()]);
    }

    #[test]
    fn spaces_at_the_break_are_trimmed() {
        let mut advance = ten_px;
        let lines = wrap_text("aa  bb", 40.0, &mut advance);
        assert_eq!(lines, vec!["aa".to_string(), "bb".to_string()]);
    }

    #[test]
    fn cjk_text_breaks_between_characters() {
        let mut advance = ten_px;
        let lines = wrap_text("中文字", 10.0, &mut advance);
        assert_eq!(
            lines,
            vec!["中".to_string(), "文".to_string(), "字".to_string()]
        );
    }

    #[test]
    fn closing_punctuation_never_starts_a_line() {
        let mut advance = ten_px;
        let lines = wrap_text("你好，世界", 30.0, &mut advance);
        assert_eq!(lines, vec!["你好，".to_string(), "世界".to_string()]);
    }

    #[test]
    fn wrapping_nothing_keeps_a_single_empty_line() {
        let mut advance = ten_px;
        assert_eq!(wrap_text("", 100.0, &mut advance), vec![String::new()]);
    }

    #[test]
    fn srgb_ui_colors_are_decoded_to_linear() {
        // #592: #FF8020 sRGB components are 1.0 / 0.502 / 0.125.
        let linear = srgb_rgba_to_linear([1.0, 0.501_960_8, 0.125, 1.0]);
        assert!((linear[0] - 1.0).abs() < 1e-6);
        assert!((linear[1] - 0.215_860_6).abs() < 1e-4);
        assert!((linear[2] - 0.014_349_9).abs() < 1e-5);
        assert_eq!(
            linear[3], 1.0,
            "Alpha does not participate in gamma conversion"
        );
        // Black / white are fixed points.
        assert_eq!(
            srgb_rgba_to_linear([0.0, 0.0, 0.0, 0.5]),
            [0.0, 0.0, 0.0, 0.5]
        );
        assert_eq!(srgb_rgba_to_linear([1.0; 4]), [1.0; 4]);
    }

    #[test]
    fn non_overlapping_glass_panels_share_one_capture() {
        let mut tracker = BackdropSegmentTracker::new(true, 1280.0, 720.0);
        let bg = crate::ui_overlay::UiQuad {
            rect: [-1.0, -1.0, 1.0, 1.0],
            ..Default::default()
        };
        let left_glass = crate::ui_overlay::UiQuad {
            rect: [-0.9, -0.8, -0.4, 0.8],
            backdrop_sigma: 20.0,
            ..Default::default()
        };
        let right_glass = crate::ui_overlay::UiQuad {
            rect: [0.4, -0.8, 0.9, 0.8],
            backdrop_sigma: 20.0,
            ..Default::default()
        };

        tracker.on_quad(&bg, 0);
        tracker.on_quad(&left_glass, 6);
        tracker.on_text([-0.85, -0.7, -0.5, -0.6]);
        tracker.on_quad(&right_glass, 18);

        let segs = tracker.finish(24);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].capture_before_sigma, None);
        assert_eq!(segs[1].capture_before_sigma, Some(20.0));
        assert_eq!(segs[1].first_vertex, 6);
        assert_eq!(segs[1].vertex_count, 18);
    }

    #[test]
    fn overlapping_glass_layers_trigger_second_capture() {
        let mut tracker = BackdropSegmentTracker::new(true, 1280.0, 720.0);
        let panel_a = crate::ui_overlay::UiQuad {
            rect: [-0.5, -0.5, 0.5, 0.5],
            backdrop_sigma: 28.0,
            ..Default::default()
        };
        let panel_b = crate::ui_overlay::UiQuad {
            rect: [-0.2, -0.2, 0.4, 0.4],
            backdrop_sigma: 16.0,
            ..Default::default()
        };

        tracker.on_quad(&panel_a, 0);
        tracker.on_text([-0.4, -0.4, 0.2, -0.3]);
        tracker.on_quad(&panel_b, 12);

        let segs = tracker.finish(18);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].capture_before_sigma, Some(28.0));
        assert_eq!(segs[1].capture_before_sigma, Some(16.0));
    }

    #[test]
    fn consecutive_border_and_inner_glass_share_one_capture() {
        let mut tracker = BackdropSegmentTracker::new(true, 1280.0, 720.0);
        let border = crate::ui_overlay::UiQuad {
            rect: [-0.3, -0.3, 0.3, 0.3],
            backdrop_sigma: 28.0,
            ..Default::default()
        };
        let inner = crate::ui_overlay::UiQuad {
            rect: [-0.29, -0.29, 0.29, 0.29],
            backdrop_sigma: 28.0,
            ..Default::default()
        };

        tracker.on_quad(&border, 0);
        tracker.on_quad(&inner, 6);

        let segs = tracker.finish(12);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].capture_before_sigma, Some(28.0));
        assert_eq!(segs[0].vertex_count, 12);
    }

    #[test]
    fn bitmap_to_sdf_produces_centered_contour() {
        let mut bmp = vec![0u8; 8 * 8];
        for y in 2..6 {
            for x in 2..6 {
                bmp[y * 8 + x] = 255;
            }
        }
        let (sdf, sw, sh) = bitmap_to_sdf(&bmp, 8, 8, 4, 4.0);
        assert_eq!((sw, sh), (16, 16));
        // Center of the filled 4x4 square should be well inside (> 128).
        assert!(sdf[8 * sw + 8] > 140);
        // Far corner should be well outside (< 128).
        assert!(sdf[0] < 80);
    }

    #[test]
    fn pixel_y_maps_screen_top_to_negative_one() {
        // Vulkan NDC y downward: pixel top is -1, pixel bottom is +1, center is 0.
        assert_eq!(pixel_y_to_ndc(0.0, 720.0), -1.0);
        assert_eq!(pixel_y_to_ndc(720.0, 720.0), 1.0);
        assert_eq!(pixel_y_to_ndc(360.0, 720.0), 0.0);
    }

    #[test]
    fn pixel_y_keeps_vertical_order() {
        // Monotonically increasing: larger pixel y must land lower (larger NDC y).
        // This is the property old implementation (`h - py`) violated.
        let ys: Vec<f32> = (0..=10)
            .map(|i| pixel_y_to_ndc(i as f32 * 72.0, 720.0))
            .collect();
        for pair in ys.windows(2) {
            assert!(
                pair[1] > pair[0],
                "NDC y must increase monotonically with pixel y"
            );
        }
    }
}
