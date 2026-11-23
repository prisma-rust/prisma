//! Backend-independent coverage tool: crop **BGRA bitmap** into 8-bit coverage bitmap for atlas.
//!
//! Only DirectWrite backend uses it (paint bitmap target white, draw glyph black, coverage = `255 - R`),
//! but here is **platform-independent** pure pixel math — so placed in standalone module, unit-testable on Linux/Android too.
//! Non-Windows builds have no callers, therefore entire module allows `dead_code`.

use super::GlyphBitmap;

/// Extract ink bounding box from 32bpp BGRA (top-down, row-major, no padding), convert to coverage bitmap.
///
/// * `origin_x` / `origin_y`: baseline origin used when drawing glyph (px, integer semantics, small numbers allowed but rounded).
/// * Second return value: whether ink touches temp bitmap edge (hint it may be clipped, caller responsible for logging).
///
/// Coverage takes **R** channel: glyph drawn anti-aliased grayscale on white background, three channels equal,
/// `coverage = 255 - R`.
pub(crate) fn crop_bgra_coverage(
    bgra: &[u8],
    box_px: u32,
    origin_x: f32,
    origin_y: f32,
    advance: f32,
) -> (GlyphBitmap, bool) {
    let box_usize = box_px as usize;
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for y in 0..box_usize {
        for x in 0..box_usize {
            // BGRA: B G R A — grayscale AA three channels equal, take R (offset 2).
            if bgra[(y * box_usize + x) * 4 + 2] < 255 {
                min_x = min_x.min(x as u32);
                min_y = min_y.min(y as u32);
                max_x = max_x.max(x as u32);
                max_y = max_y.max(y as u32);
            }
        }
    }

    // Empty white glyph (empty grid, fully corner gaps): does not occupy atlas, but advance must be kept.
    if min_x == u32::MAX {
        return (
            GlyphBitmap {
                width: 0,
                height: 0,
                advance,
                xmin: 0,
                ymin: 0,
                pixels: Vec::new(),
            },
            false,
        );
    }

    let clipped = min_x == 0 || min_y == 0 || max_x + 1 == box_px || max_y + 1 == box_px;

    let width = (max_x - min_x + 1) as usize;
    let height = (max_y - min_y + 1) as usize;
    let mut pixels = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let src = ((min_y as usize + y) * box_usize + (min_x as usize + x)) * 4 + 2;
            pixels[y * width + x] = 255u8.saturating_sub(bgra[src]);
        }
    }

    (
        GlyphBitmap {
            width,
            height,
            advance,
            xmin: min_x as i32 - origin_x.round() as i32,
            // Same meaning as fontdue `Metrics::ymin`: bitmap **bottom edge** upward offset relative baseline.
            ymin: origin_y.round() as i32 - (max_y as i32 + 1),
            pixels,
        },
        clipped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// White-background BGRA bitmap.
    fn white_box(size: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        for px in pixels.as_chunks_mut::<4>().0 {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
            px[3] = 255;
        }
        pixels
    }

    fn fill(bgra: &mut [u8], box_px: u32, x: u32, y: u32, value: u8) {
        let i = ((y * box_px + x) * 4) as usize;
        // BGRA: grayscale glyph three channels equal.
        bgra[i] = value;
        bgra[i + 1] = value;
        bgra[i + 2] = value;
    }

    #[test]
    fn blank_bitmap_keeps_advance_and_takes_no_atlas_space() {
        let (glyph, clipped) = crop_bgra_coverage(&white_box(8), 8, 4.0, 6.0, 7.5);
        assert!(!clipped);
        assert_eq!(glyph.width, 0);
        assert_eq!(glyph.height, 0);
        assert_eq!(glyph.advance, 7.5);
        assert!(glyph.pixels.is_empty());
    }

    #[test]
    fn crops_ink_box_and_maps_coverage() {
        let mut bgra = white_box(8);
        // A 2x2 ink block, upper-left corner (3, 4), grayscale 0 and 128.
        fill(&mut bgra, 8, 3, 4, 0);
        fill(&mut bgra, 8, 4, 4, 0);
        fill(&mut bgra, 8, 3, 5, 128);
        fill(&mut bgra, 8, 4, 5, 128);

        // Baseline origin (4, 6): ink left edge 1px left of origin, bottom edge 0px below baseline.
        let (glyph, clipped) = crop_bgra_coverage(&bgra, 8, 4.0, 6.0, 5.0);
        assert!(!clipped);
        assert_eq!((glyph.width, glyph.height), (2, 2));
        assert_eq!(glyph.xmin, -1);
        assert_eq!(glyph.ymin, 0);
        assert_eq!(glyph.pixels, vec![255, 255, 127, 127]);
    }

    #[test]
    fn ink_above_baseline_reports_negative_ymin() {
        let mut bgra = white_box(16);
        // A row of ink at y = 5, baseline at y = 12.
        fill(&mut bgra, 16, 5, 5, 0);

        let (glyph, _) = crop_bgra_coverage(&bgra, 16, 8.0, 12.0, 6.0);
        assert_eq!((glyph.width, glyph.height), (1, 1));
        // Bottom edge relative baseline: 12 - (5 + 1) = 6 px upward → +6 in fontdue convention.
        assert_eq!(glyph.ymin, 6);
        assert_eq!(glyph.xmin, -3);
    }

    #[test]
    fn ink_touching_the_edge_is_reported_as_clipped() {
        let mut bgra = white_box(4);
        fill(&mut bgra, 4, 0, 2, 0);
        let (_, clipped) = crop_bgra_coverage(&bgra, 4, 2.0, 2.0, 3.0);
        assert!(
            clipped,
            "Ink touching left edge must report possible clipping"
        );
    }
}
