//! The UI draw packet: what a UI producer hands to a rasterizer.
//!
//! The types here carry no widget, layout or theme semantics. A quad is a
//! rounded rectangle that may sample a bindless texture or the blurred backdrop;
//! a text run is a pixel rectangle plus a string. Producers stack the two kinds
//! in [`UiOverlayInput::order`], and the GPU side expands that order into
//! vertices.
//!
//! This crate is pure data with no dependencies, so every layer can name it:
//! the engine ECS UI, particle overlays and the editor retained UI kernel all
//! produce the same packet, and one rasterizer consumes it.
//!
//! The Vulkan implementation lives in `prism-render`, which re-exports these
//! types so a host still depends on a single rendering crate.

/// A filled rectangle in NDC space.
#[derive(Clone, Debug, PartialEq)]
pub struct UiQuad {
    /// `[xmin, ymin, xmax, ymax]` in normalized device coordinates.
    pub rect: [f32; 4],
    /// RGBA, 0..=1, interpreted as sRGB author values.
    pub color: [f32; 4],
    /// Corner radius, normalized by the quad larger side (see `prism-render`).
    pub border_radius: f32,
    /// Bindless SRV slot of a background image; `None` = flat colour.
    pub texture: Option<u32>,
    /// Backdrop blur radius in pixels; `0.0` = the quad does not sample the
    /// backdrop.
    ///
    /// The host runs one filter pass over the composited scene when any quad
    /// asks for it, then the UI shader samples the filtered result. A surface
    /// opts in by naming a backdrop-filtering material, not by a widget flag.
    pub backdrop_sigma: f32,
}

impl Default for UiQuad {
    fn default() -> Self {
        Self {
            rect: [0.0; 4],
            color: [0.0; 4],
            border_radius: 0.0,
            texture: None,
            backdrop_sigma: 0.0,
        }
    }
}

/// One text run: a screen-space pixel rectangle plus its content.
///
/// With the `ui` feature on, the renderer expands this into one quad per glyph.
#[derive(Clone, Debug)]
pub struct UiTextInput {
    /// Pixel rectangle `[left, top, width, height]`, top-left origin.
    pub rect: [f32; 4],
    pub content: String,
    pub font_size: f32,
    pub color: [f32; 4],
    pub alignment: UiTextAlign,
    /// Asset path of a project-supplied font; `None` = the system default.
    pub font: Option<String>,
    /// Wrap at the rectangle width; `false` = single line.
    pub wrap: bool,
}

/// Horizontal alignment, resolved against the measured width of the whole run.
#[derive(Clone, Copy, Debug, Default)]
pub enum UiTextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// One entry in the draw order, indexing [`UiOverlayInput::quads`] or
/// [`UiOverlayInput::texts`].
///
/// Producers fill this so rectangles and text can interleave: a full-screen
/// scrim has to cover the text drawn before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiOverlayDraw {
    Quad(u32),
    Text(u32),
}

/// One frame of UI draw commands.
#[derive(Clone, Default)]
pub struct UiOverlayInput {
    pub quads: Vec<UiQuad>,
    pub texts: Vec<UiTextInput>,
    /// Interleaved draw order. Empty degrades to "every quad, then every text".
    pub order: Vec<UiOverlayDraw>,
}

impl UiOverlayInput {
    pub fn clear(&mut self) {
        self.quads.clear();
        self.texts.clear();
        self.order.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.quads.is_empty() && self.texts.is_empty()
    }

    /// Append a quad and record its place in the draw order.
    pub fn push_quad(&mut self, quad: UiQuad) {
        self.order
            .push(UiOverlayDraw::Quad(self.quads.len() as u32));
        self.quads.push(quad);
    }

    /// Append a text run and record its place in the draw order.
    pub fn push_text(&mut self, text: UiTextInput) {
        self.order
            .push(UiOverlayDraw::Text(self.texts.len() as u32));
        self.texts.push(text);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn run(content: &str) -> UiTextInput {
        UiTextInput {
            rect: [0.0, 0.0, 10.0, 10.0],
            content: content.to_string(),
            font_size: 12.0,
            color: [1.0; 4],
            alignment: UiTextAlign::Left,
            font: None,
            wrap: false,
        }
    }

    #[test]
    fn pushing_records_the_interleaved_order() {
        let mut input = UiOverlayInput::default();
        input.push_text(run("above"));
        input.push_quad(UiQuad::default());
        input.push_text(run("below"));

        assert_eq!(
            input.order,
            vec![
                UiOverlayDraw::Text(0),
                UiOverlayDraw::Quad(0),
                UiOverlayDraw::Text(1),
            ],
            "order must follow the push sequence so a scrim can cover earlier text"
        );
        assert_eq!(input.texts.len(), 2);
        assert_eq!(input.quads.len(), 1);
    }

    #[test]
    fn clear_drops_the_commands_but_keeps_the_buffers() {
        let mut input = UiOverlayInput::default();
        input.push_quad(UiQuad::default());
        input.push_text(run("x"));
        assert!(!input.is_empty());

        let capacity = (input.quads.capacity(), input.texts.capacity());
        input.clear();
        assert!(input.is_empty());
        assert!(input.order.is_empty());
        assert_eq!(
            (input.quads.capacity(), input.texts.capacity()),
            capacity,
            "clear must not release the buffers, the packet is refilled every frame"
        );
    }
}
