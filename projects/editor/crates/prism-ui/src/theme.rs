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

            border: Color::rgb(0.196, 0.196, 0.200),
            border_strong: Color::rgb(0.290, 0.290, 0.294),
            border_subtle: Color::rgb(0.153, 0.153, 0.157),

            text: Color::rgb(0.784, 0.784, 0.788),
            text_strong: Color::rgb(0.941, 0.941, 0.945),
            text_soft: Color::rgb(0.659, 0.659, 0.663),
            muted: Color::rgb(0.478, 0.478, 0.482),

            row_hover: Color::rgb(0.173, 0.173, 0.176),
            menu_hover: Color::rgb(0.192, 0.192, 0.196),
            menu_active: Color::rgb(0.235, 0.235, 0.239),
            selection: Color::rgb(0.302, 0.223, 0.102),
            accent: Color::rgb(0.976, 0.604, 0.145),
            on_accent: Color::rgb(0.078, 0.078, 0.082),

            // Metrics are shared with the light preset; only colours and
            // corner radii differ, so a preset switch cannot relayout the UI.
            menu_bar_height: 28.0,
            menu_bar_padding: Edges::symmetric(5.0, 0.0),
            menu_bar_gap: 1.0,
            menu_trigger_padding: Edges::symmetric(8.0, 3.0),
            // Squared-off corners: this theme reads as a tool, not a page.
            menu_trigger_radius: 2.0,

            menu_dropdown_padding: Edges::symmetric(3.0, 4.0),
            menu_dropdown_radius: 3.0,
            menu_dropdown_min_width: 208.0,
            menu_dropdown_offset: 1.0,
            menu_item_height: 22.0,
            menu_item_padding: Edges::symmetric(8.0, 3.0),
            menu_item_radius: 2.0,
            menu_item_gap: 16.0,
            menu_shortcut_width: 64.0,
            menu_separator_margin: 3.0,

            font_size: 13.0,
            font_size_small: 12.0,
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

            border: Color::rgb(0.831, 0.831, 0.839),
            border_strong: Color::rgb(0.702, 0.702, 0.714),
            border_subtle: Color::rgb(0.882, 0.882, 0.890),

            text: Color::rgb(0.114, 0.114, 0.122),
            text_strong: Color::rgb(0.0, 0.0, 0.0),
            text_soft: Color::rgb(0.290, 0.290, 0.306),
            muted: Color::rgb(0.525, 0.525, 0.545),

            row_hover: Color::rgb(0.898, 0.898, 0.906),
            menu_hover: Color::rgb(0.898, 0.898, 0.906),
            menu_active: Color::rgb(0.843, 0.843, 0.855),
            selection: Color::rgb(0.796, 0.875, 1.0),
            accent: Color::rgb(0.0, 0.478, 1.0),
            on_accent: Color::rgb(1.0, 1.0, 1.0),

            // Metrics match the dark preset; generous radii are what make this
            // theme read as a system surface rather than a tool panel.
            menu_bar_height: 28.0,
            menu_bar_padding: Edges::symmetric(5.0, 0.0),
            menu_bar_gap: 1.0,
            menu_trigger_padding: Edges::symmetric(8.0, 3.0),
            menu_trigger_radius: 6.0,

            menu_dropdown_padding: Edges::symmetric(3.0, 4.0),
            menu_dropdown_radius: 9.0,
            menu_dropdown_min_width: 208.0,
            menu_dropdown_offset: 1.0,
            menu_item_height: 22.0,
            menu_item_padding: Edges::symmetric(8.0, 3.0),
            menu_item_radius: 5.0,
            menu_item_gap: 16.0,
            menu_shortcut_width: 64.0,
            menu_separator_margin: 3.0,

            font_size: 13.0,
            font_size_small: 12.0,
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
