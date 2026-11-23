//! Text rasterizer backend A: `fontdue` (all platforms, default).
//!
//! Fonts have two sources (#583):
//!
//! * **System default font**: at launch finds first available file per [`FONT_CANDIDATES`], or reads
//!   `PRISMA_UI_FONT`. When not found, does not just give up — project can bundle fonts, so instance
//!   still created; only when `font` empty and no registered font is text dropped (quad background still drawn).
//! * **Project-bundled font**: host registers `assets/fonts/*.ttf|otf` bytes via
//!   [`TextRasterizer::register_font`]; `Text.font` set to relative path to use.
//!   If path not registered / parse fails, falls back to system default font and warns once.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use fontdue::Font;

use super::{GlyphBitmap, LineMetrics, TextRasterizer};

/// Font file candidates (tried in order; ttc uses collection_index for first font).
///
/// Prefer fonts with CJK glyphs — intro title is Chinese. If all fail, falls back to "only registered project
/// fonts usable", with log::warn.
const FONT_CANDIDATES: &[(&str, u32)] = &[
    ("C:/Windows/Fonts/msyh.ttc", 0),
    ("C:/Windows/Fonts/simhei.ttf", 0),
    ("C:/Windows/Fonts/arial.ttf", 0),
    ("/system/fonts/NotoSansCJK-Regular.ttc", 0),
    ("/system/fonts/NotoSansCJKsc-Regular.otf", 0),
    ("/system/fonts/DroidSansFallback.ttf", 0),
    ("/system/fonts/Roboto-Regular.ttf", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    (
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        0,
    ),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    ("/System/Library/Fonts/PingFang.ttc", 0),
    ("/System/Library/Fonts/Helvetica.ttc", 0),
];

/// Return the first candidate for which `load` produces a value.
///
/// Missing/unreadable candidates must not abort the search: only the first
/// *available* f
fn first_available<T>(
    candidates: &[(&str, u32)],
    mut load: impl FnMut(&str, u32) -> Option<T>,
) -> Option<(String, T)> {
    for &(path, collection_index) in candidates {
        if let Some(value) = load(path, collection_index) {
            return Some((path.to_string(), value));
        }
    }
    None
}

/// Load first available system font (fontdue only accepts single font; ttc via collection_index).
///
/// `PRISMA_UI_FONT` can explicitly specify font file: on Android / slim systems candidate paths may all not
/// exist, then game should bundle a font rather than silently dropping all text.
fn load_system_font() -> Option<Font> {
    let load = |path: &str, collection_index: u32| {
        let read_start = std::time::Instant::now();
        let data = std::fs::read(path).ok()?;
        let read_ms = read_start.elapsed().as_secs_f64() * 1000.0;
        let parse_start = std::time::Instant::now();
        let font = Font::from_bytes(
            data,
            fontdue::FontSettings {
                collection_index,
                ..Default::default()
            },
        )
        .ok();
        log::debug!(
            target: "startup",
            "[startup] ui font '{path}': read {read_ms:.3}ms, parse {:.3}ms, loaded={}",
            parse_start.elapsed().as_secs_f64() * 1000.0,
            font.is_some()
        );
        font
    };

    if let Some(path) = std::env::var_os("PRISMA_UI_FONT") {
        let path = path.to_string_lossy().into_owned();
        match load(&path, 0) {
            Some(font) => {
                log::info!("ui overlay: loaded font {path} (PRISMA_UI_FONT)");
                return Some(font);
            }
            None => log::warn!(
                "ui overlay: PRISMA_UI_FONT={path} could not be loaded — falling back to system fonts"
            ),
        }
    }

    let (path, font) = first_available(FONT_CANDIDATES, load)?;
    log::info!("ui overlay: loaded font {path}");
    Some(font)
}

pub(crate) struct FontdueRasterizer {
    /// System default font; `None` means only registered project fonts usable.
    default: Option<Font>,
    /// CJK fallback font when the primary/project font lacks a glyph.
    fallback_cjk: Option<Font>,
    /// Project-bundled font: asset path (`assets/fonts/x.ttf`) → parsed font.
    fonts: HashMap<String, Font>,
    /// Set that has warned "this path not registered": `line_metrics` is `&self`, use
    /// `RefCell` so per-frame glyph lookup does not spam logs.
    reported_missing: RefCell<HashSet<String>>,
}

impl FontdueRasterizer {
    pub(crate) fn new() -> Self {
        let default = load_system_font();
        let fallback_cjk = first_available(FONT_CANDIDATES, |path, collection_index| {
            let data = std::fs::read(path).ok()?;
            let font = Font::from_bytes(
                data,
                fontdue::FontSettings {
                    collection_index,
                    ..Default::default()
                },
            )
            .ok()?;
            (font.lookup_glyph_index('中') != 0).then_some(font)
        })
        .map(|(_, f)| f);
        if default.is_none() {
            log::warn!(
                "ui overlay: no usable system UI font (tried {} candidates) — text falls back to \
                 project fonts registered via `assets/fonts/`; set PRISMA_UI_FONT to override",
                FONT_CANDIDATES.len()
            );
        }
        Self {
            default,
            fallback_cjk,
            fonts: HashMap::new(),
            reported_missing: RefCell::new(HashSet::new()),
        }
    }

    /// Font for that path; on miss warns once and falls back to default font.
    fn resolve(&self, font: Option<&str>) -> Option<&Font> {
        if let Some(name) = font {
            if let Some(found) = self.fonts.get(name) {
                return Some(found);
            }
            if self.reported_missing.borrow_mut().insert(name.to_string()) {
                log::warn!(
                    "ui overlay: font '{name}' is not registered (not in the .pak / failed to \
                     parse) — falling back to the default font"
                );
            }
        }
        self.default.as_ref().or(self.fallback_cjk.as_ref())
    }

    fn resolve_for_char(&self, font: Option<&str>, ch: char) -> Option<&Font> {
        let primary = self.resolve(font)?;
        if primary.lookup_glyph_index(ch) != 0 {
            return Some(primary);
        }
        if let Some(cjk) = self.fallback_cjk.as_ref() {
            if cjk.lookup_glyph_index(ch) != 0 {
                return Some(cjk);
            }
        }
        Some(primary)
    }
}

impl TextRasterizer for FontdueRasterizer {
    fn line_metrics(&self, font: Option<&str>, font_size: f32) -> Option<LineMetrics> {
        let font = self.resolve(font)?;
        let metrics = font.horizontal_line_metrics(font_size)?;
        Some(LineMetrics {
            ascent: metrics.ascent,
            descent: -metrics.descent,
            line_height: metrics.new_line_size,
        })
    }

    fn rasterize(&mut self, font: Option<&str>, ch: char, font_size: f32) -> Option<GlyphBitmap> {
        let found = self.resolve_for_char(font, ch)?;
        let (metrics, pixels) = found.rasterize(ch, font_size);
        Some(GlyphBitmap {
            width: metrics.width,
            height: metrics.height,
            advance: metrics.advance_width,
            xmin: metrics.xmin,
            ymin: metrics.ymin,
            pixels,
        })
    }

    fn register_font(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        match Font::from_bytes(bytes, fontdue::FontSettings::default()) {
            Ok(font) => {
                log::info!("ui overlay: registered project font {name}");
                self.fonts.insert(name.to_string(), font);
                self.reported_missing.borrow_mut().remove(name);
                true
            }
            Err(error) => {
                log::warn!("ui overlay: project font '{name}' could not be parsed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_lookup_skips_missing_candidates() {
        let candidates = [("missing", 0), ("broken", 0), ("ok", 2)];
        let found = first_available(&candidates, |path, index| match path {
            "ok" => Some(index),
            _ => None,
        });
        assert_eq!(found, Some(("ok".to_string(), 2)));
    }

    #[test]
    fn font_lookup_returns_none_when_all_candidates_fail() {
        let candidates = [("missing", 0), ("broken", 0)];
        assert!(first_available(&candidates, |_, _| None::<u32>).is_none());
    }

    #[test]
    fn registering_broken_bytes_fails_without_dropping_the_default() {
        let mut backend = FontdueRasterizer {
            default: None,
            fallback_cjk: None,
            fonts: HashMap::new(),
            reported_missing: RefCell::new(HashSet::new()),
        };
        assert!(!backend.register_font("assets/fonts/broken.ttf", vec![0, 1, 2, 3]));
        assert!(backend.fonts.is_empty());
        assert!(backend
            .rasterize(Some("assets/fonts/broken.ttf"), 'a', 16.0)
            .is_none());
        assert!(backend
            .line_metrics(Some("assets/fonts/broken.ttf"), 16.0)
            .is_none());
    }

    #[test]
    fn a_registered_font_replaces_the_previous_bytes() {
        let Some(bytes) = FONT_CANDIDATES
            .iter()
            .find_map(|(path, _)| std::fs::read(path).ok())
        else {
            return;
        };
        let mut backend = FontdueRasterizer {
            default: None,
            fallback_cjk: None,
            fonts: HashMap::new(),
            reported_missing: RefCell::new(HashSet::new()),
        };
        assert!(backend.register_font("assets/fonts/hud.ttf", bytes.clone()));
        assert!(backend
            .line_metrics(Some("assets/fonts/hud.ttf"), 24.0)
            .is_some());
        assert!(backend
            .rasterize(Some("assets/fonts/hud.ttf"), 'A', 24.0)
            .is_some());
        assert!(backend.register_font("assets/fonts/hud.ttf", bytes));
    }
}
