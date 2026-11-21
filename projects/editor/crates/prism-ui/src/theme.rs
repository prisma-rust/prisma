//! Semantic design tokens: the single source of truth for widget styling.
//!
//! Widgets never hard-code a colour or a metric; they read a [`Theme`]. Token
//! names mirror `prism_editor_core::ThemeToken` so migrating a panel from the
//! egui editor is a mechanical rename, and the two editors cannot drift apart
//! visually.
//!
//! The theme is `Copy` (only colours, floats and [`Edges`]), so element
//! builders capture it by value and no lifetime escapes a frame.

use glam::Vec2;

use crate::style::{Color, Edges};

/// Colours and metrics shared by every composed widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    // ---- surfaces (back to front) ----
    /// Window background behind every panel.
    pub bg: Color,
    /// Default panel fill.
    pub panel: Color,
    /// Alternate panel fill for striping / nested sections.
    pub panel_alt: Color,
    /// Raised surface: dropdowns, popups, tooltips.
    pub panel_raised: Color,
    /// Toolbar / menu bar chrome.
    pub chrome: Color,

    // ---- borders ----
    pub border: Color,
    /// Emphasised border (focused input, open menu).
    pub border_strong: Color,
    /// Hairline separators inside panels.
    pub border_subtle: Color,

    // ---- text ----
    pub text: Color,
    /// Emphasised text (open menu label, selected row).
    pub text_strong: Color,
    /// Secondary text that must still be readable.
    pub text_soft: Color,
    /// De-emphasised labels, shortcut hints, placeholders.
    pub muted: Color,

    // ---- interaction ----
    /// Hover fill for list rows and generic buttons.
    pub row_hover: Color,
    /// Hover fill for menu bar triggers and menu items.
    pub menu_hover: Color,
    /// Fill of an open menu bar trigger.
    pub menu_active: Color,
    /// Selection fill (selected row, toggled button).
    pub selection: Color,
    /// Accent for focus rings and active toggles.
    pub accent: Color,
    /// Text drawn on top of [`Self::accent`].
    pub on_accent: Color,

    // ---- metrics: menu bar ----
    pub menu_bar_height: f32,
    pub menu_bar_padding: Edges,
    /// Horizontal gap between menu bar triggers.
    pub menu_bar_gap: f32,
    pub menu_trigger_padding: Edges,
    pub menu_trigger_radius: f32,

    // ---- metrics: menu dropdown ----
    pub menu_dropdown_padding: Edges,
    pub menu_dropdown_radius: f32,
    /// Minimum dropdown width; it grows to fit the longest item.
    pub menu_dropdown_min_width: f32,
    /// Vertical offset between a trigger and its dropdown.
    pub menu_dropdown_offset: f32,
    pub menu_item_height: f32,
    pub menu_item_padding: Edges,
    pub menu_item_radius: f32,
    /// Gap between the label and the right-aligned shortcut hint.
    pub menu_item_gap: f32,
    /// Reserved width so shortcut hints line up in a column.
    pub menu_shortcut_width: f32,
    /// Vertical margin around a separator row.
    pub menu_separator_margin: f32,

    // ---- metrics: typography ----
    pub font_size: f32,
    pub font_size_small: f32,
}

/// Built-in colour presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemePreset {
    #[default]
    Dark,
    Light,
}

impl ThemePreset {
    /// Stable id used in menus, config files and node keys.
    pub fn id(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Human-readable name for menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    /// Every preset, in menu order.
    pub const ALL: [ThemePreset; 2] = [ThemePreset::Dark, ThemePreset::Light];
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// Dark preset; the default look of the self-hosted editor.
    pub const fn dark() -> Self {
        Self {
            bg: Color::rgb(0.086, 0.094, 0.110),
            panel: Color::rgb(0.118, 0.129, 0.149),
            panel_alt: Color::rgb(0.145, 0.157, 0.180),
            panel_raised: Color::rgb(0.180, 0.196, 0.227),
            chrome: Color::rgb(0.098, 0.106, 0.125),

            border: Color::rgb(0.239, 0.259, 0.298),
            border_strong: Color::rgb(0.353, 0.380, 0.431),
            border_subtle: Color::rgb(0.180, 0.196, 0.227),

            text: Color::rgb(0.878, 0.890, 0.914),
            text_strong: Color::rgb(0.965, 0.973, 0.984),
            text_soft: Color::rgb(0.741, 0.765, 0.804),
            muted: Color::rgb(0.541, 0.573, 0.620),

            row_hover: Color::rgb(0.200, 0.220, 0.259),
            menu_hover: Color::rgb(0.196, 0.216, 0.255),
            menu_active: Color::rgb(0.231, 0.255, 0.302),
            selection: Color::rgb(0.153, 0.267, 0.451),
            accent: Color::rgb(0.247, 0.529, 0.961),
            on_accent: Color::rgb(0.976, 0.984, 0.996),

            menu_bar_height: 30.0,
            menu_bar_padding: Edges::symmetric(6.0, 0.0),
            menu_bar_gap: 2.0,
            menu_trigger_padding: Edges::symmetric(9.0, 4.0),
            menu_trigger_radius: 4.0,

            menu_dropdown_padding: Edges::symmetric(4.0, 5.0),
            menu_dropdown_radius: 6.0,
            menu_dropdown_min_width: 208.0,
            menu_dropdown_offset: 2.0,
            menu_item_height: 24.0,
            menu_item_padding: Edges::symmetric(10.0, 3.0),
            menu_item_radius: 4.0,
            menu_item_gap: 16.0,
            menu_shortcut_width: 64.0,
            menu_separator_margin: 4.0,

            font_size: 13.0,
            font_size_small: 12.0,
        }
    }

    /// Light preset; same token set, so widgets need no conditional styling.
    pub const fn light() -> Self {
        Self {
            bg: Color::rgb(0.961, 0.965, 0.973),
            panel: Color::rgb(0.988, 0.990, 0.996),
            panel_alt: Color::rgb(0.945, 0.953, 0.965),
            panel_raised: Color::rgb(1.0, 1.0, 1.0),
            chrome: Color::rgb(0.925, 0.933, 0.945),

            border: Color::rgb(0.816, 0.831, 0.855),
            border_strong: Color::rgb(0.647, 0.671, 0.710),
            border_subtle: Color::rgb(0.894, 0.906, 0.925),

            text: Color::rgb(0.157, 0.173, 0.204),
            text_strong: Color::rgb(0.055, 0.067, 0.090),
            text_soft: Color::rgb(0.290, 0.318, 0.361),
            muted: Color::rgb(0.478, 0.510, 0.561),

            row_hover: Color::rgb(0.906, 0.925, 0.953),
            menu_hover: Color::rgb(0.898, 0.918, 0.949),
            menu_active: Color::rgb(0.855, 0.886, 0.941),
            selection: Color::rgb(0.812, 0.878, 0.976),
            accent: Color::rgb(0.145, 0.388, 0.922),
            on_accent: Color::rgb(1.0, 1.0, 1.0),

            ..Self::dark()
        }
    }

    /// Resolve a named preset.
    pub fn from_preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::Dark => Self::dark(),
            ThemePreset::Light => Self::light(),
        }
    }

    /// The preset this theme matches, decided by background brightness.
    ///
    /// The two presets differ by a wide margin there, so no tolerance is needed.
    pub fn preset(self) -> ThemePreset {
        if self.panel.r > 0.5 {
            ThemePreset::Light
        } else {
            ThemePreset::Dark
        }
    }

    /// Fill for a dropdown panel's shadow.
    pub fn dropdown_shadow(self) -> Color {
        match self.preset() {
            ThemePreset::Dark => Color::rgba(0.0, 0.0, 0.0, 0.38),
            // A hard shadow reads as a cutout on light surfaces.
            ThemePreset::Light => Color::rgba(0.06, 0.09, 0.16, 0.18),
        }
    }

    /// A size that fits the theme's standard row height.
    pub fn row_size(self, width: f32) -> Vec2 {
        Vec2::new(width, self.menu_item_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_and_light_expose_the_same_tokens() {
        let dark = Theme::dark();
        let light = Theme::light();
        // Surfaces must differ, or switching presets would be invisible.
        assert_ne!(dark.panel, light.panel);
        assert_ne!(dark.text, light.text);
        // Metrics are shared: a theme switch must not relayout the UI.
        assert_eq!(dark.menu_bar_height, light.menu_bar_height);
        assert_eq!(dark.menu_item_height, light.menu_item_height);
    }

    #[test]
    fn the_dropdown_fits_a_full_menu_item() {
        let theme = Theme::dark();
        assert!(theme.menu_dropdown_min_width > theme.menu_shortcut_width);
        assert!(theme.menu_item_height > theme.font_size);
    }
}
