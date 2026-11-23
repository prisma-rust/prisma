//! Text rasterizer backend: **who turns chars into coverage bitmaps**.
//!
//! Atlas packing, quad expansion, Vulkan upload in `ui_overlay/gpu.rs` are backend-independent; here only defines
//! backend interface ([`TextRasterizer`]) and two implementations:
//!
//! | feature | backend | platform | description |
//! |---|---|---|---|
//! | `ui-fontdue` (default) | `fontdue_backend` | all platforms | bundles ttf-parser, reads font files directly (~90 KB) |
//! | `ui-directwrite` | `directwrite` | Windows only | system `IDWriteFactory` + `IDWriteBitmapRenderTarget`, no fontdue |
//!
//! Both backends must produce **same coordinate system and metrics**, otherwise switching features makes text jitter:
//!
//! * `advance` / `xmin`: pen advance to right and bitmap left edge offset relative to pen (px).
//! * `ymin`: bitmap **bottom edge** offset relative baseline, **positive upward** (same meaning as `fontdue::Metrics::ymin`).
//!   `UiOverlay::emit_text` directly uses `baseline + ymin` as quad top edge, so both backends
//!   must interpret this field identically for consistent visuals.
//! * [`LineMetrics`]: baseline to line top / bottom / height (px), `emit_text` uses [`baseline_offset`]
//!   to derive baseline from `ascent` (not hardcoded `0.8 × font_size` approximation).
//!
//! Font sources (#583):
//!
//! * `font = None` (empty `font` in component) = system default font (`PRISMA_UI_FONT` /
//!   built-in candidate paths).
//! * `font = Some(path)` = project-bundled font, host registered bytes via [`TextRasterizer::register_font`];
//!   on miss (not registered / parse failure) falls back to system default with single warn.
//! * Atlas bucketed by `(font id, char, size)`, multiple fonts share same R8 atlas, so blending
//!   multiple fonts in UI still needs only one draw call.
//!
//! Explicitly **not doing** (both backends only draw single glyph, no complex text pipeline):
//!
//! * Arabic / Indic script shaping and top-bottom substitution (need `IDWriteTextLayout` shaping or
//!   HarfBuzz): both backends only fetch single glyph by `cmap`, such text shaping incorrect.
//! * Variable fonts (axis interpolation), Emoji color glyphs, ligatures, kerning.

/// Coverage bitmap of one already-rasterized glyph (8-bit alpha, row-major, length = `width * height`).
pub(crate) struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    /// Pen position advance (px).
    pub advance: f32,
    /// Bitmap left edge integer pixel offset relative to pen (may be negative).
    pub xmin: i32,
    /// Bitmap **bottom edge** integer pixel offset relative baseline, positive upward (see module docs).
    pub ymin: i32,
    pub pixels: Vec<u8>,
}

/// Line metrics (px, all "distances", not signed).
pub(crate) struct LineMetrics {
    /// Baseline → line top.
    pub ascent: f32,
    /// Baseline → line bottom.
    pub descent: f32,
    /// Line height (including line gap).
    pub line_height: f32,
}

/// Baseline offset of single-line text relative to text rect top edge.
///
/// When font has line metrics use `ascent`; when unavailable (font missing / metrics API failure) fall back to historical
/// `0.8 × font_size` approximation, ensuring text does not escape rect due to missing metrics.
pub(crate) fn baseline_offset(metrics: Option<LineMetrics>, font_size: f32) -> f32 {
    match metrics {
        Some(metrics) if metrics.ascent > 0.0 => metrics.ascent,
        _ => font_size * 0.8,
    }
}

/// One text rasterizer backend.
///
/// Implementation must be incrementally callable: `rasterize` called once per atlas miss, result cached by atlas.
pub(crate) trait TextRasterizer {
    /// Line metrics (px). `None` = size unavailable (caller falls back to [`baseline_offset`] approximation).
    ///
    /// `font` is project-bundled font asset path; `None` or unregistered uses system default font.
    fn line_metrics(&self, font: Option<&str>, font_size: f32) -> Option<LineMetrics>;

    /// Rasterize single char. Empty white glyph (e.g. space) returns `width == height == 0`,
    /// but still carries correct `advance`.
    fn rasterize(&mut self, font: Option<&str>, ch: char, font_size: f32) -> Option<GlyphBitmap>;

    /// Register project-bundled font (raw bytes of `.ttf` / `.otf` / `.ttc`, #583).
    ///
    /// Returns `false` when backend does not support multiple fonts or bytes parse failed; caller continues normally,
    /// text falls back to system default font.
    fn register_font(&mut self, _name: &str, _bytes: Vec<u8>) -> bool {
        false
    }
}

#[cfg(all(windows, feature = "ui-directwrite"))]
mod coverage;
#[cfg(all(windows, feature = "ui-directwrite"))]
mod directwrite;
#[cfg(feature = "ui-fontdue")]
mod fontdue_backend;

/// Construct backend by compiled-in features; `None` = only draw quads, no text.
///
/// fontdue backend returns instance even if no system font found: project-bundled fonts are registered at runtime
/// (#583), cannot permanently disable backend at startup just because machine lacks system font.
#[cfg(feature = "ui-fontdue")]
pub(crate) fn create() -> Option<Box<dyn TextRasterizer>> {
    Some(Box::new(fontdue_backend::FontdueRasterizer::new()) as Box<dyn TextRasterizer>)
}

#[cfg(all(windows, feature = "ui-directwrite", not(feature = "ui-fontdue")))]
pub(crate) fn create() -> Option<Box<dyn TextRasterizer>> {
    directwrite::DirectWriteRasterizer::new()
        .map(|backend| Box::new(backend) as Box<dyn TextRasterizer>)
}

#[cfg(not(any(feature = "ui-fontdue", all(windows, feature = "ui-directwrite"))))]
pub(crate) fn create() -> Option<Box<dyn TextRasterizer>> {
    log::warn!(
                "ui overlay: no text rasterizer backend compiled in (need feature `ui-fontdue` or `ui-directwrite`)\
                  -- UI only draws background quads, text commands ignored"
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_uses_ascent_when_available() {
        let metrics = LineMetrics {
            ascent: 22.0,
            descent: 6.0,
            line_height: 28.0,
        };
        assert_eq!(baseline_offset(Some(metrics), 24.0), 22.0);
    }

    #[test]
    fn baseline_falls_back_to_legacy_approximation() {
        assert_eq!(baseline_offset(None, 24.0), 24.0f32 * 0.8);
        let broken = LineMetrics {
            ascent: 0.0,
            descent: 0.0,
            line_height: 0.0,
        };
        assert_eq!(baseline_offset(Some(broken), 24.0), 24.0f32 * 0.8);
    }
}
