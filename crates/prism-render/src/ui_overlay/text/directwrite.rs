//! Text rasterizer backend B: Windows system DirectWrite (`ui-directwrite`).
//!
//! Purpose: Windows release packages no longer link `fontdue` + `ttf-parser` (~90 KB), instead use system
//! `dwrite.dll`. Glyph bitmaps come from `IDWriteBitmapRenderTarget` (DirectWrite's GDI interop
//! bitmap target): draw **single glyph** into a 32bpp BGRA memory bitmap, scan out ink bounding box then convert to
//! the 8-bit coverage used by the atlas.
//!
//! Why one glyph at a time instead of whole line at once: `ui_overlay::gpu`'s atlas / quad
//! expansion is **per-glyph** (alignment, advance, cache key all `(char, font_size)`). Whole-line bitmap
//! would need changing atlas cache key and alignment logic, benefit only saving a few GDI calls — not worth changing the
//! layer that has no relation to the backend. Per-glyph also makes both backends (fontdue / DirectWrite) fully comparable.
//!
//! Rendering approach:
//! * First clear bitmap white (`PatBlt(WHITENESS)`), then draw glyph in black + **grayscale** antialiasing
//!   (`CreateCustomRenderingParams(.., cleartype_level = 0.0, ..)`, ClearType level 0
//!   is single-channel grayscale AA), coverage = `255 - R`. This does not depend on bitmap alpha channel fill convention.
//! * `pixels_per_dip = 1.0`: UI coords are already pixels, scaling handled by window.
//!
//! **Limitations (same as fontdue backend)**: no complex text shaping (Arabic / Indic scripts),
//! variable fonts, Emoji color glyphs, kerning / ligatures — those still only have fontdue's
//! `rasterize(char)`-level fallback (also no shaping), or are not supported at all.

use std::mem::{size_of, ManuallyDrop};

use anyhow::{Context as _, Result};
use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteBitmapRenderTarget, IDWriteFactory, IDWriteFontCollection,
    IDWriteFontFace, IDWriteFontFamily, IDWriteGdiInterop, IDWriteRenderingParams,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS, DWRITE_GLYPH_OFFSET,
    DWRITE_GLYPH_RUN, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PIXEL_GEOMETRY_RGB,
    DWRITE_RENDERING_MODE_NATURAL,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, PatBlt, SelectObject,
    SetBkMode, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
    SRCCOPY, TRANSPARENT, WHITENESS,
};

use super::coverage::crop_bgra_coverage;
use super::{GlyphBitmap, LineMetrics, TextRasterizer};

/// Font family candidates, order aligned with fontdue's Windows candidates (msyh.ttc → simhei.ttf → arial.ttf),
/// so both backends draw same font. All missing falls back to [`FALLBACK_FAMILY`].
const FAMILY_CANDIDATES: &[&str] = &["Microsoft YaHei", "Microsoft YaHei UI", "SimHei", "Arial"];

/// Fallback font family (shipped with Windows since Vista, includes Latin but **not Chinese**).
const FALLBACK_FAMILY: &str = "Segoe UI";

/// Padding around temporary bitmap (px), avoid negative bearing / lost ink being clipped.
const PAD: u32 = 2;

pub(crate) struct DirectWriteRasterizer {
    face: IDWriteFontFace,
    /// Font design units (em unit), when converting to pixels divide by `designUnitsPerEm`.
    metrics: DWRITE_FONT_METRICS,
    interop: IDWriteGdiInterop,
    /// Grayscale AA render parameters (ClearType level = 0).
    params: IDWriteRenderingParams,
    /// Temporary bitmap target for a single glyph; `Resize` when size changes, not recreated.
    target: Option<IDWriteBitmapRenderTarget>,
    /// Current edge length of `target` (px); 0 = not yet created.
    box_px: u32,
    /// Readback surface, edge length follows `box_px`.
    readback: Option<ReadbackSurface>,
}

impl DirectWriteRasterizer {
    pub(crate) fn new() -> Option<Self> {
        match Self::try_new() {
            Ok(this) => Some(this),
            Err(err) => {
                log::warn!("ui overlay: DirectWrite backend init failed ({err:#}) — text commands will be skipped");
                None
            }
        }
    }

    fn try_new() -> Result<Self> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .context("DWriteCreateFactory")?;

        let mut collection: Option<IDWriteFontCollection> = None;
        unsafe { factory.GetSystemFontCollection(&mut collection, false) }
            .context("GetSystemFontCollection")?;
        let collection = collection.context("System font collection is empty")?;

        let (family_name, family) = pick_family(&collection)?;
        let font = unsafe {
            family.GetFirstMatchingFont(
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
        }
        .with_context(|| format!("{family_name}: GetFirstMatchingFont"))?;
        let face = unsafe { font.CreateFontFace() }
            .with_context(|| format!("{family_name}: CreateFontFace"))?;

        let mut metrics = DWRITE_FONT_METRICS::default();
        unsafe { face.GetMetrics(&mut metrics) };
        if metrics.designUnitsPerEm == 0 {
            anyhow::bail!("{family_name}: DWRITE_FONT_METRICS.designUnitsPerEm = 0");
        }

        let interop = unsafe { factory.GetGdiInterop() }.context("GetGdiInterop")?;
        let params = match unsafe {
            factory.CreateCustomRenderingParams(
                1.8,
                0.5,
                0.0,
                DWRITE_PIXEL_GEOMETRY_RGB,
                DWRITE_RENDERING_MODE_NATURAL,
            )
        } {
            Ok(params) => params,
            Err(err) => {
                log::warn!(
                    "ui overlay: Failed to create grayscale AA render params ({err}), falling back to system default params\
                     — text may have color fringes"
                );
                unsafe { factory.CreateRenderingParams() }.context("CreateRenderingParams")?
            }
        };

        log::info!("ui overlay: DirectWrite backend ready, font family {family_name}");
        Ok(Self {
            face,
            metrics,
            interop,
            params,
            target: None,
            box_px: 0,
            readback: None,
        })
    }

    /// em unit → pixels.
    fn scale(&self, font_size: f32) -> f32 {
        font_size / f32::from(self.metrics.designUnitsPerEm)
    }

    /// Get/create temporary bitmap target with edge length `box_px` (later retrieved via `self.target`).
    fn ensure_target(&mut self, box_px: u32) -> Result<()> {
        if self.box_px != box_px {
            match &self.target {
                Some(target) => unsafe { target.Resize(box_px, box_px) }
                    .with_context(|| format!("IDWriteBitmapRenderTarget::Resize({box_px})"))?,
                None => {
                    let target =
                        unsafe { self.interop.CreateBitmapRenderTarget(None, box_px, box_px) }
                            .context("CreateBitmapRenderTarget")?;
                    unsafe { target.SetPixelsPerDip(1.0) }.context("SetPixelsPerDip")?;
                    self.target = Some(target);
                }
            }
            self.box_px = box_px;
        }
        Ok(())
    }

    /// Get/create readback surface (same size as temporary bitmap).
    fn ensure_readback(&mut self, box_px: u32) -> Result<()> {
        if self.readback.as_ref().is_some_and(|rb| rb.box_px == box_px) {
            return Ok(());
        }
        self.readback = None;
        self.readback = Some(ReadbackSurface::new(box_px)?);
        Ok(())
    }

    fn try_rasterize(&mut self, ch: char, font_size: f32) -> Result<GlyphBitmap> {
        let scale = self.scale(font_size);

        let mut glyph_index = 0u16;
        let codepoint = ch as u32;
        unsafe {
            self.face
                .GetGlyphIndices(&codepoint as *const u32, 1, &mut glyph_index as *mut u16)
        }
        .with_context(|| format!("GetGlyphIndices({ch:?})"))?;
        let mut design = DWRITE_GLYPH_METRICS::default();
        unsafe {
            self.face.GetDesignGlyphMetrics(
                &glyph_index as *const u16,
                1,
                &mut design as *mut DWRITE_GLYPH_METRICS,
                false,
            )
        }
        .with_context(|| format!("GetDesignGlyphMetrics({ch:?})"))?;
        let advance = design.advanceWidth as f32 * scale;

        let em_px = font_size.ceil().max(1.0) as u32;
        let box_px = em_px * 2 + PAD * 2;
        let ascent_px = self.metrics.ascent as f32 * scale;
        let origin_x = PAD as f32 + (em_px as f32 * 0.6).round();
        let origin_y = (PAD as f32 + ascent_px.ceil()).min((box_px - 1) as f32);

        self.ensure_target(box_px)?;
        let target = self
            .target
            .as_ref()
            .context("bitmap render target unavailable")?;
        let hdc = unsafe { target.GetMemoryDC() };

        unsafe {
            PatBlt(hdc, 0, 0, box_px as i32, box_px as i32, WHITENESS)
                .ok()
                .context("PatBlt(WHITENESS) clearing temporary bitmap")?;
            SetBkMode(hdc, TRANSPARENT);
        }

        let indices = [glyph_index];
        let advances = [advance];
        let offsets = [DWRITE_GLYPH_OFFSET::default()];
        let mut run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(self.face.clone())),
            fontEmSize: font_size,
            glyphCount: 1,
            glyphIndices: indices.as_ptr(),
            glyphAdvances: advances.as_ptr(),
            glyphOffsets: offsets.as_ptr(),
            isSideways: BOOL(0),
            bidiLevel: 0,
        };
        let mut black_box = RECT::default();
        let drawn = unsafe {
            target.DrawGlyphRun(
                origin_x,
                origin_y,
                DWRITE_MEASURING_MODE_NATURAL,
                &run,
                &self.params,
                COLORREF(0),
                Some(&mut black_box),
            )
        };
        unsafe { drop(ManuallyDrop::take(&mut run.fontFace)) };
        drawn.with_context(|| format!("DrawGlyphRun({ch:?})"))?;

        self.ensure_readback(box_px)?;
        let readback = self
            .readback
            .as_ref()
            .context("Readback surface unavailable")?;
        readback.capture(hdc)?;
        let (glyph, clipped) =
            crop_bgra_coverage(readback.pixels(), box_px, origin_x, origin_y, advance);
        if clipped {
            log::warn!("ui overlay: DirectWrite glyph {ch:?} @ {font_size}px may be clipped by temporary bitmap");
        }
        Ok(glyph)
    }
}

impl TextRasterizer for DirectWriteRasterizer {
    fn line_metrics(&self, _font: Option<&str>, font_size: f32) -> Option<LineMetrics> {
        let scale = self.scale(font_size);
        let ascent = self.metrics.ascent as f32 * scale;
        let descent = self.metrics.descent as f32 * scale;
        let line_gap = self.metrics.lineGap as f32 * scale;
        Some(LineMetrics {
            ascent,
            descent,
            line_height: ascent + descent + line_gap,
        })
    }

    fn rasterize(&mut self, _font: Option<&str>, ch: char, font_size: f32) -> Option<GlyphBitmap> {
        match self.try_rasterize(ch, font_size) {
            Ok(glyph) => Some(glyph),
            Err(err) => {
                log::warn!("ui overlay: DirectWrite rasterization of {ch:?} @ {font_size}px failed: {err:#}");
                Some(GlyphBitmap {
                    width: 0,
                    height: 0,
                    advance: 0.0,
                    xmin: 0,
                    ymin: 0,
                    pixels: Vec::new(),
                })
            }
        }
    }
}

/// Bitmap readback surface: a **top-down 32bpp DIB section we create ourselves**.
///
/// Why not read `IDWriteBitmapRenderTarget`'s memory DC bitmap directly:
/// * `GetDIBits` docs require "bitmap not currently selected into DC", but this bitmap is always selected by DWrite;
/// * `BITMAP.bmBits` pointer direction depends on DIB original row order, wrong read means upside-down.
///
/// `BitBlt` between two DCs is **logical-coord to logical-coord** copy, direction determined by our own
/// negative-height DIB header, so unaffected by DWrite internal row order and does not touch GDI usage limits.
struct ReadbackSurface {
    dc: HDC,
    bitmap: HBITMAP,
    /// Default 1x1 bitmap selected into a newly created DC, must swap back before destroying.
    previous: HGDIOBJ,
    bits: *mut u8,
    box_px: u32,
}

impl ReadbackSurface {
    fn new(box_px: u32) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: box_px as i32,
                biHeight: -(box_px as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                anyhow::bail!("CreateCompatibleDC failed");
            }
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(bitmap) => bitmap,
                Err(err) => {
                    let _ = DeleteDC(dc);
                    return Err(err).context("CreateDIBSection");
                }
            };
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                anyhow::bail!("CreateDIBSection did not provide a pixel pointer");
            }
            let previous = SelectObject(dc, bitmap.into());
            Ok(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                box_px,
            })
        }
    }

    /// Copy `source` (DWrite's memory DC) over entirely.
    fn capture(&self, source: HDC) -> Result<()> {
        let size = self.box_px as i32;
        unsafe { BitBlt(self.dc, 0, 0, size, size, Some(source), 0, 0, SRCCOPY) }
            .context("BitBlt reading back glyph bitmap")
    }

    fn pixels(&self) -> &[u8] {
        let len = self.box_px as usize * self.box_px as usize * 4;
        unsafe { std::slice::from_raw_parts(self.bits, len) }
    }
}

impl Drop for ReadbackSurface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Find font family by candidate order, finally fall back to [`FALLBACK_FAMILY`].
fn pick_family(collection: &IDWriteFontCollection) -> Result<(String, IDWriteFontFamily)> {
    for name in FAMILY_CANDIDATES
        .iter()
        .copied()
        .chain(std::iter::once(FALLBACK_FAMILY))
    {
        let wide = HSTRING::from(name);
        let mut index = 0u32;
        let mut exists = BOOL(0);
        unsafe { collection.FindFamilyName(&wide, &mut index, &mut exists) }
            .with_context(|| format!("FindFamilyName({name})"))?;
        if exists.as_bool() {
            let family = unsafe { collection.GetFontFamily(index) }
                .with_context(|| format!("GetFontFamily({name})"))?;
            return Ok((name.to_string(), family));
        }
    }
    anyhow::bail!(
        "No candidate font family in system font collection ({})",
        FAMILY_CANDIDATES
            .iter()
            .chain(std::iter::once(&FALLBACK_FAMILY))
            .copied()
            .collect::<Vec<_>>()
            .join(" / ")
    )
}
