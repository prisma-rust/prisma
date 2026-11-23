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
    /// Default frosted-glass tint for floating windows and popovers.
    pub glass_tint: Color,
    /// Default backdrop blur radius (in pixels) for frosted-glass windows.
    pub glass_blur_sigma: f32,

    pub border: Color,
    /// Emphasised border (focused input, open menu).
    pub border_strong: Color,
    /// Hairline separators inside panels.
    pub border_subtle: Color,

    pub text: Color,
    /// Emphasised text (open menu label, selected row).
    pub text_strong: Color,
    /// Secondary text that must still be readable.
    pub text_soft: Color,
    /// De-emphasised labels, shortcut hints, placeholders.
    pub muted: Color,

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

    pub menu_bar_height: f32,
    pub menu_bar_padding: Edges,
    /// Horizontal gap between menu bar triggers.
    pub menu_bar_gap: f32,
    pub menu_trigger_padding: Edges,
    pub menu_trigger_radius: f32,

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

    pub font_size: f32,
    pub font_size_small: f32,
    /// Default drop-shadow offset for text runs (in pixels).
    pub text_shadow_offset: Vec2,
    /// Default drop-shadow colour for text runs.
    pub text_shadow_color: Color,
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
    /// Dark preset: neutral graphite surfaces with a warm accent.
    ///
    /// The ramp is deliberately grey — every surface shares one hue and differs
    /// only in brightness — so panels read as machined metal rather than as
    /// tinted glass. Chrome sits slightly darker than the panels it frames, and
    /// the accent is a saturated warm tone that stands clear of the grey.
    pub const fn dark() -> Self {
        Self {
            bg: Color::rgb(0.071, 0.071, 0.075),
            panel: Color::rgb(0.106, 0.106, 0.110),
            panel_alt: Color::rgb(0.129, 0.129, 0.133),
            panel_raised: Color::rgb(0.145, 0.145, 0.149),
            chrome: Color::rgb(0.086, 0.086, 0.090),
            glass_tint: Color::rgba(0.113, 0.125, 0.149, 0.72),
            glass_blur_sigma: 28.0,

            border: Color::rgb(0.196, 0.196, 0.200),
            border_strong: Color::rgb(0.290, 0.290, 0.294),
            border_subtle: Color::rgb(0.153, 0.153, 0.157),

            text: Color::WHITE,
            text_strong: Color::WHITE,
            text_soft: Color::WHITE,
            muted: Color::WHITE,

            row_hover: Color::rgb(0.173, 0.173, 0.176),
            menu_hover: Color::rgba(0.24, 0.25, 0.28, 0.65),
            menu_active: Color::rgba(0.30, 0.31, 0.35, 0.78),
            selection: Color::rgba(0.039, 0.518, 1.0, 0.28),
            accent: Color::rgb(0.039, 0.518, 1.0),
            on_accent: Color::rgb(1.0, 1.0, 1.0),

            menu_bar_height: 28.0,
            menu_bar_padding: Edges::symmetric(6.0, 0.0),
            menu_bar_gap: 2.0,
            menu_trigger_padding: Edges::symmetric(9.0, 3.0),
            menu_trigger_radius: 6.0,

            menu_dropdown_padding: Edges::symmetric(4.0, 5.0),
            menu_dropdown_radius: 9.0,
            menu_dropdown_min_width: 212.0,
            menu_dropdown_offset: 4.0,
            menu_item_height: 22.0,
            menu_item_padding: Edges::symmetric(8.0, 3.0),
            menu_item_radius: 5.0,
            menu_item_gap: 16.0,
            menu_shortcut_width: 64.0,
            menu_separator_margin: 3.0,

            font_size: 13.0,
            font_size_small: 12.0,
            text_shadow_offset: Vec2::new(0.5, 0.5),
            text_shadow_color: Color::rgb(0.5, 0.5, 0.5),
        }
    }

    /// Light preset: cool-neutral paper surfaces, blue accent, soft shadows.
    ///
    /// Surfaces sit close together in brightness and are separated by hairlines
    /// rather than by fills, which keeps a light UI from turning into a grid of
    /// boxes. The accent carries selection; hover states stay a neutral wash.
    pub const fn light() -> Self {
        Self {
            bg: Color::rgb(0.925, 0.925, 0.929),
            panel: Color::rgb(0.961, 0.961, 0.965),
            panel_alt: Color::rgb(0.910, 0.910, 0.914),
            panel_raised: Color::rgb(0.984, 0.984, 0.988),
            chrome: Color::rgb(0.937, 0.937, 0.941),
            glass_tint: Color::rgba(0.988, 0.992, 1.0, 0.78),
            glass_blur_sigma: 28.0,

            border: Color::rgb(0.831, 0.831, 0.839),
            border_strong: Color::rgb(0.702, 0.702, 0.714),
            border_subtle: Color::rgb(0.882, 0.882, 0.890),

            text: Color::BLACK,
            text_strong: Color::BLACK,
            text_soft: Color::BLACK,
            muted: Color::BLACK,

            row_hover: Color::rgb(0.898, 0.898, 0.906),
            menu_hover: Color::rgba(0.0, 0.0, 0.0, 0.08),
            menu_active: Color::rgba(0.0, 0.0, 0.0, 0.14),
            selection: Color::rgb(0.796, 0.875, 1.0),
            accent: Color::rgb(0.0, 0.478, 1.0),
            on_accent: Color::rgb(1.0, 1.0, 1.0),

            menu_bar_height: 28.0,
            menu_bar_padding: Edges::symmetric(6.0, 0.0),
            menu_bar_gap: 2.0,
            menu_trigger_padding: Edges::symmetric(9.0, 3.0),
            menu_trigger_radius: 6.0,

            menu_dropdown_padding: Edges::symmetric(4.0, 5.0),
            menu_dropdown_radius: 9.0,
            menu_dropdown_min_width: 212.0,
            menu_dropdown_offset: 4.0,
            menu_item_height: 22.0,
            menu_item_padding: Edges::symmetric(8.0, 3.0),
            menu_item_radius: 5.0,
            menu_item_gap: 16.0,
            menu_shortcut_width: 64.0,
            menu_separator_margin: 3.0,

            font_size: 13.0,
            font_size_small: 12.0,
            text_shadow_offset: Vec2::new(0.5, 0.5),
            text_shadow_color: Color::rgb(0.5, 0.5, 0.5),
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
        assert_ne!(dark.panel, light.panel);
        assert_ne!(dark.text, light.text);
        assert_eq!(dark.menu_bar_height, light.menu_bar_height);
        assert_eq!(dark.menu_item_height, light.menu_item_height);
    }

    #[test]
    fn the_dropdown_fits_a_full_menu_item() {
        let theme = Theme::dark();
        assert!(theme.menu_dropdown_min_width > theme.menu_shortcut_width);
        assert!(theme.menu_item_height > theme.font_size);
    }

    #[test]
    fn text_colors_and_neutral_gray_shadow_match_presets() {
        let dark = Theme::dark();
        let light = Theme::light();
        let expected_offset = Vec2::new(0.5, 0.5);
        let expected_shadow = Color::rgb(0.5, 0.5, 0.5);

        assert_eq!(dark.text, Color::WHITE);
        assert_eq!(dark.text_strong, Color::WHITE);
        assert_eq!(dark.text_soft, Color::WHITE);
        assert_eq!(dark.muted, Color::WHITE);
        assert_eq!(dark.text_shadow_offset, expected_offset);
        assert_eq!(dark.text_shadow_color, expected_shadow);

        assert_eq!(light.text, Color::BLACK);
        assert_eq!(light.text_strong, Color::BLACK);
        assert_eq!(light.text_soft, Color::BLACK);
        assert_eq!(light.muted, Color::BLACK);
        assert_eq!(light.text_shadow_offset, expected_offset);
        assert_eq!(light.text_shadow_color, expected_shadow);
    }

    #[test]
    fn macos_aligned_glass_tint_and_blur_preserve_readability() {
        let dark = Theme::dark();
        let light = Theme::light();

        assert_eq!(dark.glass_blur_sigma, 28.0);
        assert_eq!(light.glass_blur_sigma, 28.0);
        assert!((dark.glass_tint.a - 0.72).abs() < 1e-6);
        assert!((light.glass_tint.a - 0.78).abs() < 1e-6);
    }
}
