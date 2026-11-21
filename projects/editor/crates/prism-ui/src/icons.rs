//! Lightweight geometric & macOS-style UI icons for `prism-ui`.
//!
//! Built from SDF rounded quads and crisp glyphs so icons scale cleanly at any
//! DPI without requiring an external icon font asset.

use crate::elements::{col, label, row, text, ElementBox};
use crate::style::{Align, Color, Edges, Justify, Length};
use crate::theme::Theme;
use crate::tree::NodeKey;

/// Semantic icon kinds used by window chrome, tabs, and toolbars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Close,
    Minimize,
    Zoom,
    Play,
    Stop,
    ChevronRight,
    ChevronDown,
}

impl IconKind {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Close => "×",
            Self::Minimize => "−",
            Self::Zoom => "+",
            Self::Play => "▶",
            Self::Stop => "■",
            Self::ChevronRight => "›",
            Self::ChevronDown => "⌄",
        }
    }
}

/// macOS traffic-light palette.
pub const TRAFFIC_RED: Color = Color::rgba(1.0, 0.373, 0.341, 1.0); // #FF5F57
pub const TRAFFIC_RED_BORDER: Color = Color::rgba(0.878, 0.267, 0.243, 1.0);
pub const TRAFFIC_YELLOW: Color = Color::rgba(0.996, 0.737, 0.180, 1.0); // #FEBC2E
pub const TRAFFIC_YELLOW_BORDER: Color = Color::rgba(0.875, 0.631, 0.137, 1.0);
pub const TRAFFIC_GREEN: Color = Color::rgba(0.157, 0.784, 0.251, 1.0); // #28C840
pub const TRAFFIC_GREEN_BORDER: Color = Color::rgba(0.102, 0.671, 0.161, 1.0);
pub const TRAFFIC_GLYPH_DARK: Color = Color::rgba(0.18, 0.05, 0.04, 0.80);

/// A single 12×12 macOS circular traffic-light control.
pub fn traffic_light_dot(
    key: Option<NodeKey>,
    fill: Color,
    border: Color,
    glyph: &'static str,
    show_glyph: bool,
) -> ElementBox {
    let mut dot = col()
        .size(Length::Px(12.0), Length::Px(12.0))
        .radius(6.0)
        .border(0.75, border)
        .bg(fill)
        .align(Align::Center)
        .justify(Justify::Center);
    if let Some(k) = key {
        dot = dot.key(k).clickable();
    }
    if show_glyph {
        dot = dot.child(text(TRAFFIC_GLYPH_DARK, 10.0, glyph));
    }
    dot
}

/// Standard macOS left-aligned traffic-light cluster (Close, Minimize, Zoom/Dock).
pub fn macos_traffic_lights(
    close_key: NodeKey,
    min_key: Option<NodeKey>,
    zoom_key: Option<NodeKey>,
    show_glyphs: bool,
) -> ElementBox {
    row()
        .gap(8.0)
        .align(Align::Center)
        .padding(Edges::symmetric(2.0, 0.0))
        .child(traffic_light_dot(
            Some(close_key),
            TRAFFIC_RED,
            TRAFFIC_RED_BORDER,
            IconKind::Close.glyph(),
            show_glyphs,
        ))
        .child(traffic_light_dot(
            min_key,
            TRAFFIC_YELLOW,
            TRAFFIC_YELLOW_BORDER,
            IconKind::Minimize.glyph(),
            show_glyphs,
        ))
        .child(traffic_light_dot(
            zoom_key,
            TRAFFIC_GREEN,
            TRAFFIC_GREEN_BORDER,
            IconKind::Zoom.glyph(),
            show_glyphs,
        ))
}

/// Compact 16×16 tab close button using a crisp `×` glyph instead of a text button.
pub fn tab_close_icon(key: NodeKey, theme: &Theme) -> ElementBox {
    col()
        .key(key)
        .size(Length::Px(16.0), Length::Px(16.0))
        .radius(4.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .clickable()
        .hover_bg(theme.row_hover)
        .child(text(theme.muted, 11.0, IconKind::Close.glyph()))
}

/// Helper to build a small geometric horizontal bar (used for macOS-style separators).
pub fn hairline(color: Color) -> ElementBox {
    ElementBox::default()
        .width(Length::Percent(100.0))
        .height(Length::Px(1.0))
        .bg(color)
}

/// Render a compact icon + label pair for toolbar or tab headers.
pub fn icon_label(icon: IconKind, title: impl Into<String>, theme: &Theme) -> ElementBox {
    row()
        .gap(5.0)
        .align(Align::Center)
        .child(text(theme.muted, theme.font_size_small, icon.glyph()))
        .child(label(title))
}
