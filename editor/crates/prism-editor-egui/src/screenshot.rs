//! Editor built-in screenshot.
//!
//! Two paths:
//!
//! - **Whole window** (`window`): Windows uses `PrintWindow` + `PW_RENDERFULLCONTENT`
//!   draws window current composite into a 32bpp DIB, then converts to RGBA writes PNG. Vulkan swapchain
//!   held by egui-ash, host cannot get `VkImage`, so this is the only way to grab UI together
//!   method; samples after DWM composition, window even partially off-screen can still capture fully.
//! - **Viewport** (`scene` / `game`): engine own offscreen image (`COLOR_ATTACHMENT_OPTIMAL` layout)
//!   read back then writes PNG, does not depend on window / compositor, can output without display (CI / remote).
//!
//! Triggered by MCP (`editor_capture` in `crates/prism-editor-mcp`) or menu
//! "Window → Capture Window", both via `control.rs` local control channel.

use std::path::PathBuf;

/// What to grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// Whole editor window (UI + viewport).
    Window,
    /// Scene view engine offscreen image.
    Scene,
    /// Game view engine offscreen image.
    Game,
}

impl CaptureTarget {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "window" | "editor" => Some(Self::Window),
            "scene" | "sceneview" | "scene-view" => Some(Self::Scene),
            "game" | "gameview" | "game-view" => Some(Self::Game),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Scene => "scene",
            Self::Game => "game",
        }
    }
}

/// `1920x1080` / `1920X1080` / `1920,1080`.
pub fn parse_size(text: &str) -> Option<[u32; 2]> {
    let normalized = text.replace(['X', ','], "x");
    let (w, h) = normalized.split_once('x')?;
    let w = w.trim().parse::<u32>().ok()?;
    let h = h.trim().parse::<u32>().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some([w, h])
}

/// Default output path: `<state dir>/captures/<target>-<milliseconds>.png`.
pub fn default_path(dir: &std::path::Path, target: CaptureTarget) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    dir.join("captures")
        .join(format!("{}-{stamp}.png", target.name()))
}

/// Grab whole window client area, return `(width, height, RGBA8)`.
pub fn capture_window(hwnd: isize) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    #[cfg(target_os = "windows")]
    {
        win32::capture_client(hwnd)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
        anyhow::bail!(
            "window capture is only implemented on Windows; use `editor_capture` with \
             target=scene|game for the engine images"
        )
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    use std::ffi::c_void;

    use anyhow::{bail, Result};

    const PW_CLIENTONLY: u32 = 0x0000_0001;
    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
    const DIB_RGB_COLORS: u32 = 0;
    const BI_RGB: u32 = 0;

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pels_per_meter: i32,
        y_pels_per_meter: i32,
        clr_used: u32,
        clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 3],
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetClientRect(hwnd: isize, rect: *mut Rect) -> i32;
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
        fn PrintWindow(hwnd: isize, dc: isize, flags: u32) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(dc: isize) -> isize;
        fn CreateDIBSection(
            dc: isize,
            info: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: isize,
            offset: u32,
        ) -> isize;
        fn SelectObject(dc: isize, object: isize) -> isize;
        fn DeleteObject(object: isize) -> i32;
        fn DeleteDC(dc: isize) -> i32;
        fn GdiFlush() -> i32;
    }

    pub(super) fn capture_client(hwnd: isize) -> Result<(u32, u32, Vec<u8>)> {
        if hwnd == 0 {
            bail!("window capture: no HWND (window not created yet?)");
        }
        let mut rect = Rect::default();
        if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
            bail!("window capture: GetClientRect failed");
        }
        let width = (rect.right - rect.left).max(0) as u32;
        let height = (rect.bottom - rect.top).max(0) as u32;
        if width == 0 || height == 0 {
            bail!("window capture: window has an empty client area");
        }

        let screen_dc = unsafe { GetDC(0) };
        if screen_dc == 0 {
            bail!("window capture: GetDC(NULL) failed");
        }
        let mem_dc = unsafe { CreateCompatibleDC(screen_dc) };

        let info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: width as i32,
                height: -(height as i32),
                planes: 1,
                bit_count: 32,
                compression: BI_RGB,
                size_image: width * height * 4,
                x_pels_per_meter: 0,
                y_pels_per_meter: 0,
                clr_used: 0,
                clr_important: 0,
            },
            colors: [0; 3],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = unsafe { CreateDIBSection(screen_dc, &info, DIB_RGB_COLORS, &mut bits, 0, 0) };
        if dib == 0 || bits.is_null() {
            unsafe { DeleteDC(mem_dc) };
            unsafe { ReleaseDC(0, screen_dc) };
            bail!("window capture: CreateDIBSection failed");
        }

        let old = unsafe { SelectObject(mem_dc, dib) };
        let printed = unsafe { PrintWindow(hwnd, mem_dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT) };
        unsafe { GdiFlush() };

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if printed != 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(bits as *const u8, pixels.as_mut_ptr(), pixels.len());
            }
            prism_render::screenshot::bgra_to_rgba_in_place(&mut pixels);
        }

        unsafe {
            SelectObject(mem_dc, old);
            DeleteObject(dib);
            DeleteDC(mem_dc);
            ReleaseDC(0, screen_dc);
        }

        if printed == 0 {
            bail!("window capture: PrintWindow returned FALSE");
        }
        Ok((width, height, pixels))
    }
}

/// Grab window and write PNG, return actual path written.
pub fn save_window_png(
    hwnd: isize,
    path: &std::path::Path,
    size: Option<[u32; 2]>,
) -> anyhow::Result<PathBuf> {
    let (width, height, pixels) = capture_window(hwnd)?;
    prism_render::screenshot::save_rgba_png(path, width, height, &pixels, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parsing_accepts_common_separators() {
        assert_eq!(parse_size("1920x1080"), Some([1920, 1080]));
        assert_eq!(parse_size(" 1280X720 "), Some([1280, 720]));
        assert_eq!(parse_size("800,600"), Some([800, 600]));
        assert_eq!(parse_size("0x600"), None);
        assert_eq!(parse_size("garbage"), None);
    }

    #[test]
    fn target_parsing_is_case_insensitive() {
        assert_eq!(CaptureTarget::parse("WINDOW"), Some(CaptureTarget::Window));
        assert_eq!(
            CaptureTarget::parse("scene-view"),
            Some(CaptureTarget::Scene)
        );
        assert_eq!(CaptureTarget::parse("game"), Some(CaptureTarget::Game));
        assert_eq!(CaptureTarget::parse("viewport"), None);
    }

    #[test]
    fn default_path_lands_in_captures_dir() {
        let path = default_path(std::path::Path::new("state"), CaptureTarget::Game);
        assert!(path.starts_with(std::path::Path::new("state").join("captures")));
        assert!(path.to_string_lossy().ends_with(".png"));
        assert!(path.to_string_lossy().contains("game-"));
    }
}
