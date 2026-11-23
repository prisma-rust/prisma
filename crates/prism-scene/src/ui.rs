//! UI ECS components — component definitions for data-driven UI.
//!
//! # Philosophy
//! UI element = Entity + [`Node`] + [`Style`] + [`ComputedLayout`] + (optional [`Text`])
//! Layout System read  Style → write  ComputedLayout
//! Render System reads ComputedLayout + Text → generates draw commands

use prism_reflect::{FieldType, Reflect, ReflectError, ReflectType, Value};
use serde::{Deserialize, Serialize};

/// **Flag**: this Entity is a UI element. Layout System only queries entities with this component.
///
/// Defined as empty-brace struct (not unit struct) so scene JSON can deserialize with `{}`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Node")]
pub struct Node {}

/// Anchor corners — define child edge position relative to parent edges (0..=1 normalized).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Anchors")]
pub struct Anchors {
    /// Horizontal start corner (0 = parent left, 1 = parent right)
    pub min_x: f32,
    /// Vertical start corner (0 = parent top, 1 = parent bottom)
    pub min_y: f32,
    /// Horizontal end corner (0 = parent left, 1 = parent right)
    pub max_x: f32,
    /// Vertical end corner (0 = parent top, 1 = parent bottom)
    pub max_y: f32,
}

impl Anchors {
    /// Fill parent container
    pub const STRETCH: Self = Self {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 1.0,
        max_y: 1.0,
    };
    /// Top-left corner
    pub const TOP_LEFT: Self = Self {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 0.0,
        max_y: 0.0,
    };
    /// Center
    pub const CENTER: Self = Self {
        min_x: 0.5,
        min_y: 0.5,
        max_x: 0.5,
        max_y: 0.5,
    };
    /// Bottom center
    pub const BOTTOM_CENTER: Self = Self {
        min_x: 0.5,
        min_y: 1.0,
        max_x: 0.5,
        max_y: 1.0,
    };
}

impl Default for Anchors {
    fn default() -> Self {
        Self::STRETCH
    }
}

/// Pivot — center point for rotation / scale / positioning (normalized 0..=1).
///
/// Only **point anchors** (`Anchors` axis with `min == max`) use pivot to align size;
/// stretch axis (`min != max`) rect determined by anchors and margins, pivot not involved
/// (consistent with engine `anchorMin/anchorMax` + `pivot` semantics).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Pivot")]
pub struct Pivot {
    pub x: f32,
    pub y: f32,
}

impl Pivot {
    pub const CENTER: Self = Self { x: 0.5, y: 0.5 };
    pub const TOP_LEFT: Self = Self { x: 0.0, y: 0.0 };
}

impl Default for Pivot {
    fn default() -> Self {
        Self::CENTER
    }
}

/// Margins (pixels).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Margin")]
#[serde(default)]
pub struct Margin {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// Arrangement direction of layout group.
///
/// `None` = not a layout group: children continue positioned by own anchors (legacy behavior).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutDirection {
    /// Don't arrange children, position by anchors (default).
    #[default]
    None,
    /// Arrange horizontally (left → right).
    Row,
    /// Arrange vertically (top → bottom).
    Column,
}

impl ReflectType for LayoutDirection {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["None", "Row", "Column"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::None => 0,
            Self::Row => 1,
            Self::Column => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::None),
            1 => Ok(Self::Row),
            2 => Ok(Self::Column),
            other => Err(ReflectError::BadData(format!(
                "invalid LayoutDirection discriminant {other}"
            ))),
        }
    }
}

/// Alignment of children on **cross axis** inside layout group.
///
/// Main axis (`Row` horizontal / `Column` vertical) determined by arrangement order + `spacing`,
/// children occupy space from content-area start; cross axis uses value here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildAlign {
    /// Stick to content-area start (default).
    #[default]
    Start,
    /// Center.
    Center,
    /// Stick to content-area end.
    End,
    /// Stretch across cross axis.
    Stretch,
}

impl ReflectType for ChildAlign {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Start", "Center", "End", "Stretch"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Start => 0,
            Self::Center => 1,
            Self::End => 2,
            Self::Stretch => 3,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Start),
            1 => Ok(Self::Center),
            2 => Ok(Self::End),
            3 => Ok(Self::Stretch),
            other => Err(ReflectError::BadData(format!(
                "invalid ChildAlign discriminant {other}"
            ))),
        }
    }
}

/// Layout group: arrange children sequentially by direction (#527).
///
/// Corresponds to basic shape of Unity `HorizontalLayoutGroup` / `VerticalLayoutGroup`:
/// only `direction`, `spacing`, `padding`, `childAlign` and ContentSizeFitter-style
/// `fit_content`, no grow / shrink / weights.
///
/// Semantics:
/// - Child size on main axis from its own `width` (`Row`) or `height` (`Column`),
///   when absent use content size measured as `fit_content` container, else 0;
/// - Cross-axis size defaults to child's own width / height (or content size), `child_align = Stretch`
///   stretches to content area;
/// - `padding` shrinks content area from container rect, children positioned relative to content area;
/// - `fit_content` shrinks container on axes with **no explicit width / height** to content size
///   (+ corresponding padding on both sides).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Layout")]
#[serde(default)]
pub struct Layout {
    /// Arrangement direction; `None` = not a layout group.
    pub direction: LayoutDirection,
    /// Spacing between adjacent children (px).
    pub spacing: f32,
    /// Content-area inner padding (px).
    pub padding: Margin,
    /// Cross-axis alignment.
    pub child_align: ChildAlign,
    /// Shrink axes with no explicit size to content size.
    pub fit_content: bool,
}

/// Visual and layout properties of a UI element.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Style")]
pub struct Style {
    /// Fixed width (px). `None` = determined by anchors.
    #[serde(default)]
    pub width: Option<f32>,
    /// Fixed height (px). `None` = determined by anchors.
    #[serde(default)]
    pub height: Option<f32>,
    /// Anchors.
    #[serde(default)]
    pub anchors: Anchors,
    /// Pivot (normalized, 0..=1).
    #[serde(default)]
    pub pivot: Pivot,
    /// Margin offsets (px).
    #[serde(default)]
    pub margin: Margin,
    /// Background color (RGBA, 0..=1). `[0;4]` = transparent.
    ///
    /// With `image` configured this acts as **tint**: `alpha == 0` treated as opaque white,
    /// displaying texture original colors (#591).
    #[serde(default)]
    pub background: [f32; 4],
    /// Background image (path relative to `assets/`; empty = none, only draw `background` solid color).
    ///
    /// Texture fills element rect with nearest sampling (consistent with pixel art); `Button`'s
    /// interaction color likewise multiplied as tint, so image can brighten / darken with button (#591).
    #[serde(default)]
    pub image: String,
    /// Border corner radius (px).
    #[serde(default)]
    pub border_radius: f32,
    /// Visibility.
    #[serde(default = "default_visible")]
    pub visible: bool,
    /// Layout group (#527). Default `direction = None`, children positioned by anchors, behavior unchanged.
    #[serde(default)]
    pub layout: Layout,
}

/// serde default of `Style::visible`.
///
/// Must stay consistent with [`Style::default`]: when author JSON omits `visible` element is visible,
/// must not fall to `bool::default()` (`false`).
fn default_visible() -> bool {
    true
}

impl Default for Style {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            anchors: Anchors::STRETCH,
            pivot: Pivot::CENTER,
            margin: Margin::default(),
            background: [0.0; 4],
            image: String::new(),
            border_radius: 0.0,
            visible: true,
            layout: Layout::default(),
        }
    }
}

impl Style {
    /// Create a fullscreen-filling transparent UI element.
    pub fn fullscreen() -> Self {
        Self::default()
    }

    /// Create a fixed-size UI element (anchors centered).
    pub fn fixed(w: f32, h: f32) -> Self {
        Self {
            width: Some(w),
            height: Some(h),
            anchors: Anchors::CENTER,
            pivot: Pivot::CENTER,
            ..Default::default()
        }
    }
}

/// **Layout System output**: final screen-space rect (pixels).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::ComputedLayout")]
pub struct ComputedLayout {
    /// screen space boundary  `[left, top, width, height]`
    #[serde(default)]
    pub rect: [f32; 4],
    /// Layer-level draw order (depth-first pre-order: parent before, child after, siblings by document order).
    ///
    /// Recomputed every frame by layout system, a **derived value** like `rect` — author doesn't write it, `.scene.json`
    /// has no such field. Background and text of same entity share same `order` (draw list in [`crate::ui`]
    /// distinguishes "rect first then text"), so sibling z order is what layer-level panel shows
    /// in the layer-level panel (see #462).
    #[serde(default)]
    pub order: u32,
    /// Scale factor of Canvas this node is in (#463). `rect` is already **screen pixels**,
    /// this value only converts "design pixel" quantities like font size / corner radius to screen pixels.
    /// `1.0` = no Canvas (legacy behavior).
    #[serde(default = "identity_scale")]
    pub scale: f32,
}

/// serde default of `ComputedLayout::scale`: `1.0` (no scaling).
pub(crate) fn identity_scale() -> f32 {
    1.0
}

impl ComputedLayout {
    /// Layout with rect only (order left 0, scale 1, filled later by layout system).
    pub fn from_rect(rect: [f32; 4]) -> Self {
        Self {
            rect,
            order: 0,
            scale: 1.0,
        }
    }

    pub fn left(&self) -> f32 {
        self.rect[0]
    }
    pub fn top(&self) -> f32 {
        self.rect[1]
    }
    pub fn width(&self) -> f32 {
        self.rect[2]
    }
    pub fn height(&self) -> f32 {
        self.rect[3]
    }
}

/// Text content.
///
/// `#[serde(default)]`: legacy documents or hand-edited JSON can write only `content`,
/// missing fields fall back to [`Text::default`], not fail whole component deserialization.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Text")]
#[serde(default)]
pub struct Text {
    pub content: String,
    pub font_size: f32,
    pub color: [f32; 4],
    pub alignment: TextAlign,
    /// Project-bundled font (path relative to `assets/fonts/*.ttf|otf|ttc`, empty = system default font, #583).
    ///
    /// Font file is normal asset: cooker recognizes `AssetType::Font` and packs as-is into `.pak`,
    /// at runtime read bytes via `ResourceManager` and hand to UI rasterizer.
    pub font: String,
    /// Auto wrap by text rect width (#527); `false` = single line (legacy behavior).
    ///
    /// Wrap breaks at spaces, by char when a single word exceeds width; `\n` always breaks.
    #[serde(default)]
    pub wrap: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            content: String::new(),
            font_size: 16.0,
            color: [1.0; 4],
            alignment: TextAlign::Center,
            font: String::new(),
            wrap: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Center,
    Left,
    Right,
}

impl ReflectType for TextAlign {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Center", "Left", "Right"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Center => 0,
            Self::Left => 1,
            Self::Right => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Center),
            1 => Ok(Self::Left),
            2 => Ok(Self::Right),
            other => Err(ReflectError::BadData(format!(
                "invalid TextAlign discriminant {other}"
            ))),
        }
    }
}

/// Canvas scale mode (corresponds to Unity `CanvasScaler` three Screen Match Modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanvasScaleMode {
    /// 1 screen pixel = 1 design pixel (default = legacy behavior with no Canvas).
    #[default]
    ConstantPixelSize,
    /// Scale by reference resolution: width and height each compute a ratio, then interpolate by
    /// [`Canvas::match_width_or_height`] in between.
    ScaleWithScreenSize,
    /// Use only width or height: `match_width_or_height < 0.5` takes width, otherwise height.
    MatchWidthOrHeight,
}

impl ReflectType for CanvasScaleMode {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&[
            "ConstantPixelSize",
            "ScaleWithScreenSize",
            "MatchWidthOrHeight",
        ])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::ConstantPixelSize => 0,
            Self::ScaleWithScreenSize => 1,
            Self::MatchWidthOrHeight => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::ConstantPixelSize),
            1 => Ok(Self::ScaleWithScreenSize),
            2 => Ok(Self::MatchWidthOrHeight),
            other => Err(ReflectError::BadData(format!(
                "invalid CanvasScaleMode discriminant {other}"
            ))),
        }
    }
}

/// UI canvas: maps "design resolution" to actual screen, scales whole subtree (#463).
///
/// Attached to a root node, its child nodes convert by [`Canvas::scale_factor`]:
/// layout computed in **virtual resolution**, multiplied back by scale when writing `ComputedLayout.rect`,
/// so `Style.width` / `margin` / anchors written in design numbers, 1080p and 720p
/// see same layout. Absolute quantities like font size and corner radius scaled by
/// render system via `ComputedLayout.scale`.
///
/// Nested Canvas doesn't establish multiple virtual coordinate systems: child inherits nearest Canvas only
/// (different from Unity nesting behavior, documented in document).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Canvas")]
#[serde(default)]
pub struct Canvas {
    /// Design resolution (e.g. 1920×1080).
    pub reference_resolution: [f32; 2],
    pub scale_mode: CanvasScaleMode,
    /// `0` = fully match width, `1` = fully match height.
    pub match_width_or_height: f32,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            reference_resolution: [1920.0, 1080.0],
            scale_mode: CanvasScaleMode::ScaleWithScreenSize,
            match_width_or_height: 0.5,
        }
    }
}

impl Canvas {
    /// Scale factor screen pixels / design pixels.
    ///
    /// Degenerate input (0 or non-finite) treated as 1, avoid whole UI becoming NaN.
    pub fn scale_factor(&self, screen: (f32, f32)) -> f32 {
        let width = finite_positive(screen.0, 1.0);
        let height = finite_positive(screen.1, 1.0);
        let reference_width = finite_positive(self.reference_resolution[0], 1920.0);
        let reference_height = finite_positive(self.reference_resolution[1], 1080.0);
        let by_width = width / reference_width;
        let by_height = height / reference_height;
        let factor = match self.scale_mode {
            CanvasScaleMode::ConstantPixelSize => 1.0,
            CanvasScaleMode::MatchWidthOrHeight => {
                if self.match_width_or_height >= 0.5 {
                    by_height
                } else {
                    by_width
                }
            }
            CanvasScaleMode::ScaleWithScreenSize => {
                let t = match self.match_width_or_height {
                    t if t.is_finite() => t.clamp(0.0, 1.0),
                    _ => 0.5,
                };
                (by_width.ln() * (1.0 - t) + by_height.ln() * t).exp()
            }
        };
        finite_positive(factor, 1.0)
    }
}

fn finite_positive(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

/// Interaction state (updated each frame by Input System).
///
/// Since #528 can also attach in scene: state rewritten each frame but component serializable,
/// so scene / prefab / template can save "this element is interactive", game code
/// need not insert runtime component itself.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Interaction")]
#[serde(default)]
pub struct Interaction {
    pub hovered: bool,
    /// Pointer **pressed on target** (dragging out of element releases, no longer counted as pressed).
    pub pressed: bool,
    /// One-frame valid: set when pressed inside element and released inside (or keyboard activated).
    pub clicked: bool,
}

/// Clickable button's **appearance** state colors (#528).
///
/// Pure data: `ui_render_system` picks color by [`Interaction`], `ui_input_system`
/// handles hover / pressed / clicked semantics. UI elements without `Button` keep using
/// [`Style::background`], both can coexist (`Button` takes priority).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::ui::Button")]
#[serde(default)]
pub struct Button {
    /// `false` = display [`Button::disabled`] color, no longer respond to clicks.
    pub interactable: bool,
    pub normal: [f32; 4],
    pub hover: [f32; 4],
    pub pressed: [f32; 4],
    pub disabled: [f32; 4],
}

impl Default for Button {
    fn default() -> Self {
        Self {
            interactable: true,
            normal: [0.18, 0.20, 0.24, 1.0],
            hover: [0.24, 0.27, 0.32, 1.0],
            pressed: [0.12, 0.14, 0.17, 1.0],
            disabled: [0.15, 0.15, 0.16, 0.85],
        }
    }
}

impl Button {
    /// Background color to draw now; when `interaction` missing treated as non-interactive.
    pub fn color(&self, interaction: Option<&Interaction>) -> [f32; 4] {
        if !self.interactable {
            return self.disabled;
        }
        match interaction {
            Some(state) if state.pressed => self.pressed,
            Some(state) if state.hovered => self.hover,
            _ => self.normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Button, Canvas, CanvasScaleMode, ChildAlign, ComputedLayout, Interaction, Layout,
        LayoutDirection, Margin, Style, Text, TextAlign,
    };
    use prism_reflect::ReflectType;

    #[test]
    fn button_colors_follow_the_interaction_state() {
        let button = Button::default();
        assert_eq!(button.color(None), button.normal);
        assert_eq!(
            button.color(Some(&Interaction {
                hovered: true,
                ..Default::default()
            })),
            button.hover
        );
        assert_eq!(
            button.color(Some(&Interaction {
                hovered: true,
                pressed: true,
                ..Default::default()
            })),
            button.pressed,
            "Pressed takes priority over hover"
        );
        let disabled = Button {
            interactable: false,
            ..Default::default()
        };
        assert_eq!(
            disabled.color(Some(&Interaction {
                hovered: true,
                pressed: true,
                ..Default::default()
            })),
            disabled.disabled
        );
    }

    #[test]
    fn button_and_interaction_round_trip_through_json() {
        let button: Button = serde_json::from_str(r#"{"interactable":false}"#).unwrap();
        assert!(!button.interactable);
        assert_eq!(
            button.normal,
            Button::default().normal,
            "Other fields use defaults"
        );
        let json = serde_json::to_string(&button).unwrap();
        let back: Button = serde_json::from_str(&json).unwrap();
        assert!(!back.interactable);

        let interaction: Interaction =
            serde_json::from_str(r#"{"hovered":true,"pressed":false,"clicked":true}"#).unwrap();
        assert!(interaction.hovered && interaction.clicked && !interaction.pressed);
        let legacy: Interaction = serde_json::from_str("{}").unwrap();
        assert!(!legacy.hovered, "Empty object = not interacted");
    }

    #[test]
    fn style_partial_json_is_visible_by_default() {
        let style: Style = serde_json::from_str(r#"{"width":100}"#).unwrap();
        assert_eq!(style.width, Some(100.0));
        assert!(
            style.visible,
            "omitting `visible` must not hide the element"
        );
    }

    #[test]
    fn text_font_defaults_to_the_system_font_and_round_trips() {
        let text: Text = serde_json::from_str(r#"{"content":"hi","font_size":20}"#).unwrap();
        assert!(text.font.is_empty());

        let authored: Text = serde_json::from_str(
            r#"{"content":"hi","font_size":20,"font":"assets/fonts/hud.ttf"}"#,
        )
        .unwrap();
        assert_eq!(authored.font, "assets/fonts/hud.ttf");
        let json = serde_json::to_string(&authored).unwrap();
        let back: Text = serde_json::from_str(&json).unwrap();
        assert_eq!(back.font, "assets/fonts/hud.ttf");
    }

    #[test]
    fn canvas_scales_the_reference_resolution_to_the_screen() {
        let canvas = Canvas::default();
        assert_eq!(canvas.scale_mode, CanvasScaleMode::ScaleWithScreenSize);
        assert!((canvas.scale_factor((960.0, 540.0)) - 0.5).abs() < 1e-5);
        assert!((canvas.scale_factor((1920.0, 1080.0)) - 1.0).abs() < 1e-5);
        assert!((canvas.scale_factor((3840.0, 2160.0)) - 2.0).abs() < 1e-5);

        let skewed = canvas.scale_factor((960.0, 1080.0));
        assert!((skewed - 0.5f32.sqrt()).abs() < 1e-4, "{skewed}");

        let width_only = Canvas {
            scale_mode: CanvasScaleMode::MatchWidthOrHeight,
            match_width_or_height: 0.0,
            ..Canvas::default()
        };
        assert!((width_only.scale_factor((960.0, 1080.0)) - 0.5).abs() < 1e-5);
        let height_only = Canvas {
            scale_mode: CanvasScaleMode::MatchWidthOrHeight,
            match_width_or_height: 1.0,
            ..Canvas::default()
        };
        assert!((height_only.scale_factor((960.0, 1080.0)) - 1.0).abs() < 1e-5);

        let pixels = Canvas {
            scale_mode: CanvasScaleMode::ConstantPixelSize,
            ..Canvas::default()
        };
        assert_eq!(pixels.scale_factor((640.0, 360.0)), 1.0);
        assert_eq!(pixels.scale_factor((0.0, f32::NAN)), 1.0);
    }

    #[test]
    fn canvas_round_trips_and_legacy_layouts_default_to_scale_one() {
        let canvas = Canvas {
            reference_resolution: [1280.0, 720.0],
            scale_mode: CanvasScaleMode::MatchWidthOrHeight,
            match_width_or_height: 1.0,
        };
        let json = serde_json::to_string(&canvas).unwrap();
        let back: Canvas = serde_json::from_str(&json).unwrap();
        assert_eq!(back.reference_resolution, [1280.0, 720.0]);
        assert_eq!(back.scale_mode, CanvasScaleMode::MatchWidthOrHeight);

        let layout: ComputedLayout =
            serde_json::from_str(r#"{"rect":[0,0,100,50],"order":3}"#).unwrap();
        assert_eq!(layout.scale, 1.0);
    }

    #[test]
    fn style_empty_object_matches_default() {
        let style: Style = serde_json::from_str("{}").unwrap();
        assert_eq!(style.width, Style::default().width);
        assert!(style.visible);
    }

    #[test]
    fn style_explicit_invisible_is_respected() {
        let style: Style = serde_json::from_str(r#"{"visible":false}"#).unwrap();
        assert!(!style.visible);
    }

    #[test]
    fn text_partial_json_fills_defaults() {
        let text: Text = serde_json::from_str(r#"{"content":"hello"}"#).unwrap();
        assert_eq!(text.content, "hello");
        assert_eq!(text.font_size, 16.0);
        assert_eq!(text.color, [1.0; 4]);
        assert!(matches!(text.alignment, TextAlign::Center));
    }

    #[test]
    fn text_empty_object_is_default() {
        let text: Text = serde_json::from_str("{}").unwrap();
        assert!(text.content.is_empty());
        assert_eq!(text.font_size, Text::default().font_size);
    }

    #[test]
    fn layout_defaults_to_no_grouping_and_round_trips() {
        let legacy: Style = serde_json::from_str(r#"{"width":100}"#).unwrap();
        assert_eq!(legacy.layout.direction, LayoutDirection::None);
        assert!(!legacy.layout.fit_content);

        let authored: Style = serde_json::from_str(
            r#"{"layout":{"direction":"Row","spacing":8,"padding":{"left":4,"top":2},
                "child_align":"Stretch","fit_content":true}}"#,
        )
        .unwrap();
        assert_eq!(authored.layout.direction, LayoutDirection::Row);
        assert_eq!(authored.layout.spacing, 8.0);
        assert_eq!(authored.layout.padding.left, 4.0);
        assert_eq!(authored.layout.padding.top, 2.0);
        assert_eq!(authored.layout.child_align, ChildAlign::Stretch);
        assert!(authored.layout.fit_content);

        let json = serde_json::to_string(&Layout::default()).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back.direction, LayoutDirection::None);
        assert_eq!(back.padding.left, Margin::default().left);
    }

    #[test]
    fn layout_enums_expose_reflect_variants() {
        assert_eq!(
            LayoutDirection::enum_variants(),
            Some(&["None", "Row", "Column"][..])
        );
        assert_eq!(
            ChildAlign::enum_variants(),
            Some(&["Start", "Center", "End", "Stretch"][..])
        );
        assert_eq!(
            LayoutDirection::from_value(prism_reflect::Value::U64(2)).unwrap(),
            LayoutDirection::Column
        );
        assert_eq!(
            ChildAlign::from_value(prism_reflect::Value::U64(3)).unwrap(),
            ChildAlign::Stretch
        );
        assert!(LayoutDirection::from_value(prism_reflect::Value::U64(9)).is_err());
    }

    #[test]
    fn text_wrap_defaults_to_off_and_round_trips() {
        let text: Text = serde_json::from_str(r#"{"content":"a b"}"#).unwrap();
        assert!(!text.wrap);
        let authored: Text = serde_json::from_str(r#"{"content":"a b","wrap":true}"#).unwrap();
        assert!(authored.wrap);
    }
}
