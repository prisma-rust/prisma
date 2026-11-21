//! Lucide SVG icon registry and macOS-style window control widgets.
//!
//! Every icon is defined as an official 24×24 Lucide SVG string, rasterized to
//! an RGBA8 white-alpha mask by [`crate::svg::rasterize_lucide_svg`], and
//! uploaded to the renderer's bindless texture table (`IconSet`).

use std::collections::BTreeMap;

use crate::draw::UiTextureId;
use crate::elements::{col, row, ElementBox};
use crate::style::{Align, Color, Edges, Justify, Length};
use crate::theme::Theme;
use crate::tree::NodeKey;

/// Semantic icon kinds used by window chrome, menus, and toolbars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IconKind {
    Close,
    Minimize,
    Zoom,
    Check,
    Play,
    Stop,
    Undo,
    Redo,
    ChevronRight,
    ChevronDown,
    Move,
    Rotate,
    Scale,
    Search,
}

impl IconKind {
    pub const ALL: [Self; 14] = [
        Self::Close,
        Self::Minimize,
        Self::Zoom,
        Self::Check,
        Self::Play,
        Self::Stop,
        Self::Undo,
        Self::Redo,
        Self::ChevronRight,
        Self::ChevronDown,
        Self::Move,
        Self::Rotate,
        Self::Scale,
        Self::Search,
    ];

    /// Stable upload key used between `editor-app` and the renderer.
    pub fn key(self) -> &'static str {
        match self {
            Self::Close => "lucide:x",
            Self::Minimize => "lucide:minus",
            Self::Zoom => "lucide:plus",
            Self::Check => "lucide:check",
            Self::Play => "lucide:play",
            Self::Stop => "lucide:square",
            Self::Undo => "lucide:undo-2",
            Self::Redo => "lucide:redo-2",
            Self::ChevronRight => "lucide:chevron-right",
            Self::ChevronDown => "lucide:chevron-down",
            Self::Move => "lucide:move",
            Self::Rotate => "lucide:rotate-cw",
            Self::Scale => "lucide:maximize-2",
            Self::Search => "lucide:search",
        }
    }

    /// Official 24×24 Lucide SVG markup for this icon.
    pub fn lucide_svg(self) -> &'static str {
        match self {
            Self::Close => {
                r#"<svg viewBox="0 0 24 24"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#
            }
            Self::Minimize => r#"<svg viewBox="0 0 24 24"><path d="M5 12h14"/></svg>"#,
            Self::Zoom => {
                r#"<svg viewBox="0 0 24 24"><path d="M5 12h14"/><path d="M12 5v14"/></svg>"#
            }
            Self::Check => r#"<svg viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5"/></svg>"#,
            Self::Play => {
                r#"<svg viewBox="0 0 24 24"><polygon points="6 3 20 12 6 21 6 3"/></svg>"#
            }
            Self::Stop => {
                r#"<svg viewBox="0 0 24 24"><rect width="14" height="14" x="5" y="5" rx="2"/></svg>"#
            }
            Self::Undo => {
                r#"<svg viewBox="0 0 24 24"><path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/></svg>"#
            }
            Self::Redo => {
                r#"<svg viewBox="0 0 24 24"><path d="m15 14 5-5-5-5"/><path d="M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13"/></svg>"#
            }
            Self::ChevronRight => r#"<svg viewBox="0 0 24 24"><path d="m9 18 6-6-6-6"/></svg>"#,
            Self::ChevronDown => r#"<svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>"#,
            Self::Move => {
                r#"<svg viewBox="0 0 24 24"><path d="M5 9l-3 3 3 3"/><path d="M9 5l3-3 3 3"/><path d="M15 19l-3 3-3-3"/><path d="M19 9l3 3-3 3"/><path d="M2 12h20"/><path d="M12 2v20"/></svg>"#
            }
            Self::Rotate => {
                r#"<svg viewBox="0 0 24 24"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>"#
            }
            Self::Scale => {
                r#"<svg viewBox="0 0 24 24"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" x2="14" y1="3" y2="10"/><line x1="3" x2="10" y1="21" y2="14"/></svg>"#
            }
            Self::Search => {
                r#"<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/></svg>"#
            }
        }
    }
}

/// Maps each [`IconKind`] to its uploaded GPU bindless texture slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconSet {
    textures: BTreeMap<IconKind, UiTextureId>,
}

impl IconSet {
    pub fn insert(&mut self, kind: IconKind, texture: UiTextureId) {
        self.textures.insert(kind, texture);
    }

    pub fn get(&self, kind: IconKind) -> Option<UiTextureId> {
        self.textures.get(&kind).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

/// Mount a rasterized Lucide SVG icon at `size × size` tinted with `tint`.
pub fn icon_view(icons: &IconSet, kind: IconKind, size: f32, tint: Color) -> ElementBox {
    let mut el = ElementBox::default().size(Length::Px(size), Length::Px(size));
    if let Some(tex) = icons.get(kind) {
        el = el.image(tex, tint);
    }
    el
}

/// macOS traffic-light palette.
pub const TRAFFIC_RED: Color = Color::rgba(1.0, 0.373, 0.341, 1.0); // #FF5F57
pub const TRAFFIC_RED_BORDER: Color = Color::rgba(0.878, 0.267, 0.243, 1.0);
pub const TRAFFIC_YELLOW: Color = Color::rgba(0.996, 0.737, 0.180, 1.0); // #FEBC2E
pub const TRAFFIC_YELLOW_BORDER: Color = Color::rgba(0.875, 0.631, 0.137, 1.0);
pub const TRAFFIC_GREEN: Color = Color::rgba(0.157, 0.784, 0.251, 1.0); // #28C840
pub const TRAFFIC_GREEN_BORDER: Color = Color::rgba(0.102, 0.671, 0.161, 1.0);
pub const TRAFFIC_GLYPH_DARK: Color = Color::rgba(0.18, 0.05, 0.04, 0.85);

/// A single 12×12 macOS circular traffic-light control.
pub fn traffic_light_dot(
    icons: &IconSet,
    key: Option<NodeKey>,
    fill: Color,
    border: Color,
    icon: IconKind,
    show_icon: bool,
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
    if show_icon {
        dot = dot.child(icon_view(icons, icon, 8.0, TRAFFIC_GLYPH_DARK));
    }
    dot
}

/// Standard macOS left-aligned traffic-light cluster (Close, Minimize, Zoom/Dock).
pub fn macos_traffic_lights(
    icons: &IconSet,
    close_key: NodeKey,
    min_key: Option<NodeKey>,
    zoom_key: Option<NodeKey>,
    show_icons: bool,
) -> ElementBox {
    row()
        .gap(8.0)
        .align(Align::Center)
        .padding(Edges::symmetric(2.0, 0.0))
        .child(traffic_light_dot(
            icons,
            Some(close_key),
            TRAFFIC_RED,
            TRAFFIC_RED_BORDER,
            IconKind::Close,
            show_icons,
        ))
        .child(traffic_light_dot(
            icons,
            min_key,
            TRAFFIC_YELLOW,
            TRAFFIC_YELLOW_BORDER,
            IconKind::Minimize,
            show_icons,
        ))
        .child(traffic_light_dot(
            icons,
            zoom_key,
            TRAFFIC_GREEN,
            TRAFFIC_GREEN_BORDER,
            IconKind::Zoom,
            show_icons,
        ))
}

/// Compact 16×16 tab close button using the Lucide `x` SVG icon.
pub fn tab_close_icon(icons: &IconSet, key: NodeKey, theme: &Theme) -> ElementBox {
    col()
        .key(key)
        .size(Length::Px(16.0), Length::Px(16.0))
        .radius(4.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .clickable()
        .hover_bg(theme.row_hover)
        .child(icon_view(icons, IconKind::Close, 10.0, theme.muted))
}

/// Helper to build a 1px horizontal separator hairline.
pub fn hairline(color: Color) -> ElementBox {
    ElementBox::default()
        .width(Length::Percent(100.0))
        .height(Length::Px(1.0))
        .bg(color)
}
