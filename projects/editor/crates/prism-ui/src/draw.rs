//! Platform-agnostic UI draw list ready for `prism-render` (`UiOverlayInput` / Render Graph).

use glam::Vec2;

use crate::style::{Color, Rect};

/// Bindless SRV texture slot or registered offscreen viewport texture handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UiTextureId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiDrawCommand {
    /// Filled rounded rectangle, optionally textured (e.g. icons or zero-copy 3D viewports).
    Quad {
        rect: Rect,
        color: Color,
        border_color: Color,
        border_width: f32,
        radius: f32,
        texture: Option<UiTextureId>,
    },
    /// Crisp text run in screen-space pixel coordinates (top-left origin).
    Text {
        rect: Rect,
        content: String,
        font_size: f32,
        color: Color,
        align: TextAlign,
        wrap: bool,
    },
    /// Push a scissor clip rect (intersected with parent clip).
    PushClip(Rect),
    PopClip,
    /// Vector line segment for node-graph wires, gizmo overlays, or timelines.
    Line {
        a: Vec2,
        b: Vec2,
        width: f32,
        color: Color,
    },
    /// Render-graph Dual-Kawase backdrop blur pass over the given rect.
    #[cfg(feature = "ui-blur")]
    BackdropBlur {
        rect: Rect,
        radius: f32,
        blur_sigma: f32,
        tint: Color,
    },
    /// Analytical SDF drop shadow pass.
    #[cfg(feature = "ui-effects")]
    DropShadow {
        rect: Rect,
        radius: f32,
        color: Color,
        offset: Vec2,
        blur: f32,
    },
}

/// Ordered per-frame UI draw buffer emitted by [`crate::tree::UiTree::build_draw_list`].
#[derive(Clone, Debug, Default)]
pub struct UiDrawList {
    pub commands: Vec<UiDrawCommand>,
}

impl UiDrawList {
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn push(&mut self, cmd: UiDrawCommand) {
        self.commands.push(cmd);
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}
