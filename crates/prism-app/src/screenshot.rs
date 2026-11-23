//! Built-in screenshot: write frame to PNG via env var at a certain frame.
//!
//! Minimal interface for automation (CI screenshots, editor / MCP capture, art tuning),
//! no new public API introduced:
//!
//! - `PRISMA_SCREENSHOT=<path>` enable, write PNG (parent dir must exist).
//! - `PRISMA_SCREENSHOT_FRAME=<n>` grab at frame n (default 90, give resource upload and animation time).
//! - `PRISMA_SCREENSHOT_SIZE=1920x1080` specify output resolution.
//! - `PRISMA_SCREENSHOT_SCALE=2` scale by window resolution multiple (`SIZE` priority).
//!
//! Scaling is nearest-neighbor, pixel art scaling won't blur; content itself is still that window frame, no re-render.

use std::path::PathBuf;

/// One pending screenshot.
#[derive(Debug, Clone)]
pub struct ScreenshotPlan {
    pub path: PathBuf,
    pub frame: u64,
    pub size: Option<[u32; 2]>,
    pub scale: Option<f32>,
}

impl ScreenshotPlan {
    /// Read from env var; returns `None` when no `PRISMA_SCREENSHOT`.
    pub fn from_env() -> Option<Self> {
        let raw_path = std::env::var("PRISMA_SCREENSHOT").ok()?;
        let trimmed = raw_path.trim();
        if trimmed.is_empty() {
            return None;
        }
        let path = PathBuf::from(trimmed);
        let frame = std::env::var("PRISMA_SCREENSHOT_FRAME")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|frame| *frame > 0)
            .unwrap_or(90);
        let size = std::env::var("PRISMA_SCREENSHOT_SIZE")
            .ok()
            .and_then(|value| parse_size(&value));
        let scale = std::env::var("PRISMA_SCREENSHOT_SCALE")
            .ok()
            .and_then(|value| value.trim().parse::<f32>().ok())
            .filter(|scale| *scale > 0.0);
        if size.is_none() && scale.is_none() {
            log::debug!("screenshot: no size/scale requested, using the window resolution");
        }
        Some(Self {
            path,
            frame,
            size,
            scale,
        })
    }

    /// Whether this frame reaches or exceeds set frame count (once satisfied and content ready can grab).
    pub fn fires_on(&self, frame_count: u64) -> bool {
        frame_count >= self.frame
    }
}

/// Parse `1920x1080` / `1920X1080` / `1280,720`.
fn parse_size(raw: &str) -> Option<[u32; 2]> {
    let normalized = raw.trim().replace(['X', ',', '*'], "x");
    let (width, height) = normalized.split_once('x')?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    (width > 0 && height > 0).then_some([width, height])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_size_spellings() {
        assert_eq!(parse_size("1920x1080"), Some([1920, 1080]));
        assert_eq!(parse_size(" 1280X720 "), Some([1280, 720]));
        assert_eq!(parse_size("800,600"), Some([800, 600]));
        assert_eq!(parse_size("0x600"), None);
        assert_eq!(parse_size("wide"), None);
    }

    #[test]
    fn fires_when_frame_reached() {
        let plan = ScreenshotPlan {
            path: PathBuf::from("shot.png"),
            frame: 5,
            size: None,
            scale: None,
        };
        assert!(!plan.fires_on(4));
        assert!(plan.fires_on(5));
        assert!(plan.fires_on(6));
    }
}
