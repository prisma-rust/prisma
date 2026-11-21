//! UI overlay: the draw packet plus its Vulkan rasterizer.
//!
//! The packet types (quads, text runs, draw order) live in `prism-ui-draw` and
//! are re-exported here, so a host that only produces UI commands can depend on
//! that crate instead of the renderer, and every producer in the engine and the
//! editor hands the rasterizer the same structure.
//!
//! The GPU half (font atlas, pipeline, drawing) is in `gpu.rs` and is gated on
//! the `ui` feature. With `ui` off this module is only the packet types; the engine
//! still fills them and the render side ignores them.

pub use prism_ui_draw::UiOverlayDraw;
pub use prism_ui_draw::UiOverlayInput;
pub use prism_ui_draw::UiQuad;
pub use prism_ui_draw::UiTextAlign;
pub use prism_ui_draw::UiTextInput;

#[cfg(feature = "ui")]
mod gpu;
#[cfg(feature = "ui")]
pub(crate) mod text;
#[cfg(feature = "ui")]
pub use gpu::UiOverlay;

// ---------------------------------------------------------------------------
// Text backend feature combinations
// ---------------------------------------------------------------------------
//
// The two backends are mutually exclusive: enabling both leaves fontdue and
// ttf-parser in the binary while the author believes the switch took effect.
// Failing the build is better than silently shipping the extra 90 KB.

// `ui-fontdue` and `ui-directwrite` are alternatives, never both.
#[cfg(all(feature = "ui-fontdue", feature = "ui-directwrite"))]
compile_error!(
    "features `ui-fontdue` and `ui-directwrite` are mutually exclusive: pick one text 
     rasterizer.\nWindows release builds save ~90 KB with `default-features = false, 
     features = [\"ui\", \"ui-directwrite\"]`; every other platform uses `\"ui-fontdue\"`."
);

// DirectWrite is a Windows-only backend; other targets have no `windows` bindings.
#[cfg(all(feature = "ui-directwrite", not(windows)))]
compile_error!(
    "feature `ui-directwrite` is only available on Windows targets because it links the 
     `windows` DirectWrite/GDI bindings. Use `ui-fontdue` on other targets, or pick no 
     text backend to draw quads without text."
);
