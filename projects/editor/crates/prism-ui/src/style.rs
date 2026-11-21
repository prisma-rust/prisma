//! Visual styling, layout tokens, and interaction-state resolution.

use glam::{Vec2, Vec4};

use crate::material::Material;

/// Normalized RGBA color (`[0.0, 1.0]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::rgba(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::rgba(0.0, 0.0, 0.0, 1.0);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let v = Vec4::from(self.to_array()).lerp(Vec4::from(other.to_array()), t.clamp(0.0, 1.0));
        Self::rgba(v.x, v.y, v.z, v.w)
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

/// Axis-aligned pixel rectangle (top-left origin).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }

    pub fn intersect(self, other: Self) -> Self {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        Self {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0.0),
            h: (y1 - y0).max(0.0),
        }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

/// Per-edge spacing (padding / margin).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Edges {
    pub const fn all(v: f32) -> Self {
        Self {
            left: v,
            right: v,
            top: v,
            bottom: v,
        }
    }

    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }

    pub fn horizontal(self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
    Grow(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PositionType {
    #[default]
    Relative,
    Absolute,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutStyle {
    pub direction: FlexDirection,
    pub position: PositionType,
    pub left: Length,
    pub top: Length,
    pub width: Length,
    pub height: Length,
    pub min_width: f32,
    pub min_height: f32,
    pub max_width: f32,
    pub max_height: f32,
    pub padding: Edges,
    pub gap: f32,
    pub align_items: Align,
    pub justify_content: Justify,
    pub clip_children: bool,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            direction: FlexDirection::Row,
            position: PositionType::Relative,
            left: Length::Auto,
            top: Length::Auto,
            width: Length::Auto,
            height: Length::Auto,
            min_width: 0.0,
            min_height: 0.0,
            max_width: f32::INFINITY,
            max_height: f32::INFINITY,
            padding: Edges::default(),
            gap: 0.0,
            align_items: Align::Stretch,
            justify_content: Justify::Start,
            clip_children: false,
        }
    }
}

/// Bitflags describing the live interaction state of a UI node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub selected: bool,
    pub disabled: bool,
}

#[cfg(feature = "ui-effects")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowStyle {
    pub color: Color,
    pub offset: Vec2,
    pub blur: f32,
    pub spread: f32,
}

/// SDF outline and drop-shadow parameters for text runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextEffect {
    pub outline_width: f32,
    pub outline_color: Color,
    pub shadow_offset: Vec2,
    pub shadow_color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VisualStyle {
    pub bg: Option<Color>,
    pub border_color: Option<Color>,
    pub border_width: f32,
    pub radius: f32,
    pub opacity: f32,
    /// Shader and parameters that produce this node's surface.
    ///
    /// Widgets set a material instead of piling up look-specific fields: a
    /// frosted panel, a textured sprite and a project shader all travel the
    /// same path.
    pub material: Option<Material>,
    pub text_effect: TextEffect,
    #[cfg(feature = "ui-effects")]
    pub shadow: Option<ShadowStyle>,
}

impl Default for VisualStyle {
    fn default() -> Self {
        Self {
            bg: None,
            border_color: None,
            border_width: 0.0,
            radius: 0.0,
            opacity: 1.0,
            material: None,
            text_effect: TextEffect::default(),
            #[cfg(feature = "ui-effects")]
            shadow: None,
        }
    }
}

impl VisualStyle {
    fn apply_patch(&mut self, patch: &VisualPatch) {
        if let Some(bg) = patch.bg {
            self.bg = Some(bg);
        }
        if let Some(bc) = patch.border_color {
            self.border_color = Some(bc);
        }
        if let Some(bw) = patch.border_width {
            self.border_width = bw;
        }
        if let Some(r) = patch.radius {
            self.radius = r;
        }
        if let Some(o) = patch.opacity {
            self.opacity = o;
        }
        if let Some(material) = &patch.material {
            self.material = Some(material.clone());
        }
    }
}

/// Partial style override applied when an interaction state is active.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisualPatch {
    pub bg: Option<Color>,
    pub border_color: Option<Color>,
    pub border_width: Option<f32>,
    pub radius: Option<f32>,
    pub opacity: Option<f32>,
    pub material: Option<Material>,
}

/// Uniform state-driven visual style: one resolution rule for all widgets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisualBox {
    pub base: VisualStyle,
    pub hover: Option<VisualPatch>,
    pub pressed: Option<VisualPatch>,
    pub focused: Option<VisualPatch>,
    pub selected: Option<VisualPatch>,
    pub disabled: Option<VisualPatch>,
}

impl VisualBox {
    pub fn resolve(&self, state: &InteractionState) -> VisualStyle {
        let mut out = self.base.clone();
        if state.disabled {
            if let Some(p) = &self.disabled {
                out.apply_patch(p);
            }
            return out;
        }
        if state.selected {
            if let Some(p) = &self.selected {
                out.apply_patch(p);
            }
        }
        if state.focused {
            if let Some(p) = &self.focused {
                out.apply_patch(p);
            }
        }
        if state.hovered {
            if let Some(p) = &self.hover {
                out.apply_patch(p);
            }
        }
        if state.pressed {
            if let Some(p) = &self.pressed {
                out.apply_patch(p);
            }
        }
        out
    }
}
