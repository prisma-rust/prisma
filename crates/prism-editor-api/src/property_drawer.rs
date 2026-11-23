//! Schema projection of `PropertyDrawer` / `PropertyAttribute` shapes.
//!
//! Here [`Range`] / [`TextArea`] / [`ColorUsage`] such **attributes** are
//! rendered by corresponding `PropertyDrawer` into slider / multi-line text box / color picker; `[CustomPropertyDrawer]` hands
//! some field or some type entirely to custom drawer.
//!
//! This layer only outputs **declarative DTO** (renderer + attribute list), actual UI in `editor/`'s Vue
//! ([`FieldEditor.vue`](../../../editor/src/components/FieldEditor.vue)). This gives
//! engine extend experience while keeping `prism-editor-api` free of UI / engine / Vulkan link.
//!
//! | | Here |
//! |---|---|
//! | `[Range(min, max)]` | [`PropertyDrawer::range`] → [`PropertyRenderer::Slider`] |
//! | `[Min]` / `[Max]` | [`PropertyDrawer::min`] / [`PropertyDrawer::max`] |
//! | `[Multiline(lines)]` | [`PropertyDrawer::multiline`] → [`PropertyRenderer::TextArea`] |
//! | `[TextArea(min, max)]` | [`PropertyDrawer::text_area`] |
//! | `[ColorUsage(showAlpha, hdr)]` | [`PropertyDrawer::color_usage`] → [`PropertyRenderer::Color`] |
//! | `[Header]` / `[Tooltip]` / `[Delayed]` | [`PropertyDrawer::with_header`] / [`PropertyDrawer::with_tooltip`] / [`PropertyDrawer::delayed`] |
//! | `[CustomPropertyDrawer(typeof(X))]` | [`PropertyDrawerRegistry::register_type`] |
//! | `[CustomPropertyDrawer]` to some field | [`PropertyDrawerRegistry::register_custom`] |
//!
//! Built-in component drawer declarations in [`PropertyDrawerRegistry::with_builtin`], applied by
//! [`crate::schema::build_schema`]; host can also build own registry and append declarations with
//! [`crate::schema::build_schema_with`].

use serde::{Deserialize, Serialize};

/// Field renderer = which `PropertyDrawer` selected here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyRenderer {
    /// Default control by `FieldSchema.kind` (text box / checkbox / dropdown / number box).
    #[default]
    Default,
    /// Slider for `[Range]`.
    Slider,
    /// Multi-line text box for `[TextArea]` / `[Multiline]`.
    TextArea,
    /// Color picker for `[ColorUsage]` (forced even if field name doesn't look like color).
    Color,
    /// `[CustomPropertyDrawer]`: find frontend component by [`PropertyDrawer::custom`] name.
    Custom,
}

/// One attribute (`PropertyAttribute`).
///
/// Serialized into tagged object like `{ "type": "range", "min": 0.0, "max": 5.0 }`,
/// frontend dispatches by `type`, ignores unknown type (keep forward compatibility).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PropertyAttribute {
    /// `[Range(min, max)]`: slider + number box.
    Range { min: f32, max: f32 },
    /// `[Min(min)]`: lower limit (no slider shown).
    Min { min: f32 },
    /// `[Max(max)]`: upper limit (no slider shown).
    Max { max: f32 },
    /// `[Multiline(lines)]`: fixed-line multi-line text box.
    Multiline { lines: usize },
    /// `[TextArea(minLines, maxLines)]`: resizable multi-line text box.
    TextArea { min_lines: usize, max_lines: usize },
    /// `[ColorUsage(showalpha , hdr)]`: whether color picker shows alpha / supports HDR.
    ColorUsage { show_alpha: bool, hdr: bool },
    /// `[Header(text)]`: group title above field.
    Header { text: String },
    /// `[Tooltip(text)]`: hover hint.
    Tooltip { text: String },
    /// `[Delayed]`: commit only on Enter / blur, not on every input.
    Delayed,
}

/// Draw declaration for one field (renderer + attribute list).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyDrawer {
    /// Which drawer to select.
    pub renderer: PropertyRenderer,
    /// Attribute list (order = declaration order).
    pub attributes: Vec<PropertyAttribute>,
    /// Frontend component name when `renderer == Custom` (like `TransformEditor`).
    pub custom: Option<String>,
}

impl PropertyDrawer {
    /// Open an empty declaration with given renderer.
    pub fn new(renderer: PropertyRenderer) -> Self {
        Self {
            renderer,
            attributes: Vec::new(),
            custom: None,
        }
    }

    /// `[Range(min, max)]` → slider. When `max <= min` treated invalid and falls back to default control.
    pub fn range(min: f32, max: f32) -> Self {
        if !(min.is_finite() && max.is_finite() && max > min) {
            return Self::default();
        }
        Self::new(PropertyRenderer::Slider).attribute(PropertyAttribute::Range { min, max })
    }

    /// `[Min(min)]`: only lower limit.
    pub fn min(min: f32) -> Self {
        Self::new(PropertyRenderer::Default).attribute(PropertyAttribute::Min { min })
    }

    /// `[Max(max)]`: only upper limit.
    pub fn max(max: f32) -> Self {
        Self::new(PropertyRenderer::Default).attribute(PropertyAttribute::Max { max })
    }

    /// `[Multiline(lines)]` → multi-line text box.
    pub fn multiline(lines: usize) -> Self {
        Self::new(PropertyRenderer::TextArea).attribute(PropertyAttribute::Multiline {
            lines: lines.max(1),
        })
    }

    /// `[TextArea(minLines, maxLines)]` → resizable multi-line text box.
    pub fn text_area(min_lines: usize, max_lines: usize) -> Self {
        let min_lines = min_lines.max(1);
        let max_lines = max_lines.max(min_lines);
        Self::new(PropertyRenderer::TextArea).attribute(PropertyAttribute::TextArea {
            min_lines,
            max_lines,
        })
    }

    /// `[ColorUsage(showalpha , hdr)]` → color picker.
    pub fn color_usage(show_alpha: bool, hdr: bool) -> Self {
        Self::new(PropertyRenderer::Color)
            .attribute(PropertyAttribute::ColorUsage { show_alpha, hdr })
    }

    /// `[CustomPropertyDrawer]`: hand to frontend component named `renderer`.
    pub fn custom(renderer: impl Into<String>) -> Self {
        Self {
            renderer: PropertyRenderer::Custom,
            attributes: Vec::new(),
            custom: Some(renderer.into()),
        }
    }

    /// Append one attribute.
    pub fn attribute(mut self, attribute: PropertyAttribute) -> Self {
        // Duplicate same class attribute keeps only last (only one Range/Tooltip).
        self.attributes.retain(|existing| {
            std::mem::discriminant(existing) != std::mem::discriminant(&attribute)
        });
        self.attributes.push(attribute);
        self
    }

    /// `[Header(text)]`.
    pub fn with_header(self, text: impl Into<String>) -> Self {
        self.attribute(PropertyAttribute::Header { text: text.into() })
    }

    /// `[Tooltip(text)]`.
    pub fn with_tooltip(self, text: impl Into<String>) -> Self {
        self.attribute(PropertyAttribute::Tooltip { text: text.into() })
    }

    /// `[Delayed]`.
    pub fn delayed(self) -> Self {
        self.attribute(PropertyAttribute::Delayed)
    }

    /// Upper/lower bounds of `[Range]` (`None` if absent).
    pub fn range_bounds(&self) -> Option<(f32, f32)> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Range { min, max } => Some((*min, *max)),
                _ => None,
            })
    }

    /// Lower limit (`[Range]` or `[Min]`).
    pub fn lower_bound(&self) -> Option<f32> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Range { min, .. } | PropertyAttribute::Min { min } => Some(*min),
                _ => None,
            })
    }

    /// Upper limit (`[Range]` or `[Max]`).
    pub fn upper_bound(&self) -> Option<f32> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Range { max, .. } | PropertyAttribute::Max { max } => Some(*max),
                _ => None,
            })
    }

    /// Line count range of multi-line text box.
    pub fn line_range(&self) -> Option<(usize, usize)> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Multiline { lines } => Some((*lines, *lines)),
                PropertyAttribute::TextArea {
                    min_lines,
                    max_lines,
                } => Some((*min_lines, *max_lines)),
                _ => None,
            })
    }

    /// `(show_alpha, hdr)` of `[ColorUsage]`.
    pub fn color_usage_flags(&self) -> Option<(bool, bool)> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::ColorUsage { show_alpha, hdr } => Some((*show_alpha, *hdr)),
                _ => None,
            })
    }

    /// `[Tooltip]` text.
    pub fn tooltip(&self) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Tooltip { text } => Some(text.as_str()),
                _ => None,
            })
    }

    /// `[Header]` text.
    pub fn header(&self) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Header { text } => Some(text.as_str()),
                _ => None,
            })
    }

    /// Whether has `[Delayed]`.
    pub fn is_delayed(&self) -> bool {
        self.attributes
            .iter()
            .any(|attribute| matches!(attribute, PropertyAttribute::Delayed))
    }
}

/// Scope of one draw declaration.
#[derive(Debug, Clone, PartialEq)]
struct DrawerEntry {
    /// Declaring type (canonical name of `#[reflect(name = ...)]`).
    type_name: String,
    /// `Some(field)` = only that field; `None` = whole type
    /// (`[CustomPropertyDrawer(typeof(X))]`).
    field: Option<String>,
    drawer: PropertyDrawer,
}

/// Field drawer registry = lookup table of `PropertyDrawer`.
///
/// Lookup order: **field-level declarations take priority over type-level**; same class declaration registered later overrides earlier
/// (consistent with derived / later-set attribute override).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropertyDrawerRegistry {
    entries: Vec<DrawerEntry>,
}

impl PropertyDrawerRegistry {
    /// Empty registry (when no declarations schema `drawer` all `None`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Drawer declarations for built-in component fields (used by [`crate::schema::build_schema`]).
    ///
    /// Only declare fields with **explicit numeric / display semantics**, don't guess controls for all fields:
    /// `Camera` exposure / fov / background color, `Light` intensity / range / spot angle / shadow strength,
    /// `Skybox` rotation / intensity, `Text` foreground.
    pub fn with_builtin() -> Self {
        let mut registry = Self::new();

        let camera = "prism_engine::scene::Camera";
        registry.register(
            camera,
            "fov_y_degrees",
            PropertyDrawer::range(1.0, 179.0)
                .with_tooltip("Vertical field of view in degrees = Camera.fieldOfView"),
        );
        registry.register(
            camera,
            "exposure",
            PropertyDrawer::range(0.0, 5.0).with_tooltip("Exposure multiplier, 1.0 = no scaling"),
        );
        registry.register(
            camera,
            "background_color",
            PropertyDrawer::color_usage(true, false)
                .with_tooltip("Solid color background (effective when clearFlags = SolidColor)"),
        );
        registry.register(camera, "orthographic_size", PropertyDrawer::min(0.0001));
        registry.register(camera, "near", PropertyDrawer::min(0.0001));
        registry.register(camera, "far", PropertyDrawer::min(0.0001));

        let light = "prism_engine::scene::Light";
        registry.register(light, "intensity", PropertyDrawer::min(0.0));
        registry.register(light, "range", PropertyDrawer::min(0.0));
        registry.register(light, "spot_angle", PropertyDrawer::range(1.0, 179.0));
        registry.register(light, "inner_spot_angle", PropertyDrawer::range(1.0, 179.0));
        registry.register(
            light,
            "shadow_strength",
            PropertyDrawer::range(0.0, 1.0)
                .with_tooltip("Shadow strength, 0 = no shadow, 1 = fully black"),
        );
        registry.register(light, "color_temperature", PropertyDrawer::min(0.0));

        let skybox = "prism_engine::scene::Skybox";
        registry.register(
            skybox,
            "rotation_degrees",
            PropertyDrawer::range(0.0, 360.0),
        );
        registry.register(skybox, "intensity", PropertyDrawer::min(0.0));

        registry.register(
            "prism_engine::ui::Text",
            "content",
            PropertyDrawer::text_area(2, 6),
        );

        registry
    }

    /// Register drawer for some field of some type = `[CustomPropertyDrawer]` to field.
    pub fn register(
        &mut self,
        type_name: impl Into<String>,
        field: impl Into<String>,
        drawer: PropertyDrawer,
    ) {
        let type_name = type_name.into();
        let field = field.into();
        self.entries.retain(|entry| {
            !(entry.type_name == type_name && entry.field.as_deref() == Some(field.as_str()))
        });
        self.entries.push(DrawerEntry {
            type_name,
            field: Some(field),
            drawer,
        });
    }

    /// Register drawer for whole type = `[CustomPropertyDrawer(typeof(X))]`.
    ///
    /// Field-level declaration still overrides it (attribute on field takes priority over type-level drawer).
    pub fn register_type(&mut self, type_name: impl Into<String>, drawer: PropertyDrawer) {
        let type_name = type_name.into();
        self.entries
            .retain(|entry| !(entry.type_name == type_name && entry.field.is_none()));
        self.entries.push(DrawerEntry {
            type_name,
            field: None,
            drawer,
        });
    }

    /// Register custom frontend component for some field (`[CustomPropertyDrawer]`).
    pub fn register_custom(
        &mut self,
        type_name: impl Into<String>,
        field: impl Into<String>,
        renderer: impl Into<String>,
    ) {
        self.register(type_name, field, PropertyDrawer::custom(renderer));
    }

    /// Look up which drawer some field should use.
    ///
    /// Lookup order:
    /// 1. field-level declaration (`declaring_type` + `field`);
    /// 2. field value type-level declaration (`value_type`, `[CustomPropertyDrawer(typeof(X))]`).
    pub fn drawer_for_field(
        &self,
        declaring_type: &str,
        field: &str,
        value_type: Option<&str>,
    ) -> Option<&PropertyDrawer> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.type_name == declaring_type && entry.field.as_deref() == Some(field)
            })
            .or_else(|| {
                value_type.and_then(|val_ty| {
                    self.entries
                        .iter()
                        .rev()
                        .find(|entry| entry.type_name == val_ty && entry.field.is_none())
                })
            })
            .map(|entry| &entry.drawer)
    }

    /// Look up concrete field of some declaring type (without passing value type).
    pub fn drawer_for(&self, type_name: &str, field: &str) -> Option<&PropertyDrawer> {
        self.drawer_for_field(type_name, field, None)
    }

    /// Look up drawer targeting some type itself (`[CustomPropertyDrawer(typeof(X))]`).
    pub fn drawer_for_type(&self, type_name: &str) -> Option<&PropertyDrawer> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.type_name == type_name && entry.field.is_none())
            .map(|entry| &entry.drawer)
    }

    /// Number of registered declarations (for tests / diagnostics).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no declarations at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_sets_slider_and_bounds() {
        let drawer = PropertyDrawer::range(0.0, 5.0).with_tooltip("exposure");
        assert_eq!(drawer.renderer, PropertyRenderer::Slider);
        assert_eq!(drawer.range_bounds(), Some((0.0, 5.0)));
        assert_eq!(drawer.lower_bound(), Some(0.0));
        assert_eq!(drawer.upper_bound(), Some(5.0));
        assert_eq!(drawer.tooltip(), Some("exposure"));
    }

    #[test]
    fn invalid_range_falls_back_to_default() {
        let drawer = PropertyDrawer::range(2.0, 2.0);
        assert_eq!(drawer.renderer, PropertyRenderer::Default);
        assert!(drawer.attributes.is_empty());
    }

    #[test]
    fn text_area_and_multiline_report_line_ranges() {
        assert_eq!(PropertyDrawer::multiline(3).line_range(), Some((3, 3)));
        assert_eq!(PropertyDrawer::text_area(2, 6).line_range(), Some((2, 6)));
        // min > max normalized to max = min.
        assert_eq!(PropertyDrawer::text_area(4, 1).line_range(), Some((4, 4)));
        assert_eq!(PropertyDrawer::text_area(0, 0).line_range(), Some((1, 1)));
    }

    #[test]
    fn duplicate_attributes_keep_the_last_one() {
        let drawer = PropertyDrawer::range(0.0, 1.0)
            .with_tooltip("first")
            .with_tooltip("second");
        assert_eq!(drawer.tooltip(), Some("second"));
        // Range only one.
        assert_eq!(
            drawer
                .attributes
                .iter()
                .filter(|a| matches!(a, PropertyAttribute::Range { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn delayed_flag_is_detected() {
        assert!(!PropertyDrawer::range(0.0, 1.0).is_delayed());
        assert!(PropertyDrawer::range(0.0, 1.0).delayed().is_delayed());
    }

    #[test]
    fn lookup_prefers_field_scope_over_type_scope() {
        let mut registry = PropertyDrawerRegistry::new();
        // Register type-level drawer for type demo::X (`[CustomPropertyDrawer(typeof(X))]`)
        registry.register_type("demo::X", PropertyDrawer::custom("WholeType"));

        // Host has value field of type demo::X: hits type-level drawer
        assert_eq!(
            registry
                .drawer_for_field("demo::Host", "x_field", Some("demo::X"))
                .map(|d| d.renderer),
            Some(PropertyRenderer::Custom)
        );

        // Register field-level drawer for specific field: overrides type-level
        registry.register("demo::Host", "x_field", PropertyDrawer::range(0.0, 1.0));
        assert_eq!(
            registry
                .drawer_for_field("demo::Host", "x_field", Some("demo::X"))
                .map(|d| d.renderer),
            Some(PropertyRenderer::Slider)
        );

        // Other same-type fields of same host still use type-level declaration
        assert_eq!(
            registry
                .drawer_for_field("demo::Host", "other_x", Some("demo::X"))
                .and_then(|d| d.custom.as_deref()),
            Some("WholeType")
        );

        // Fields of other value types not affected
        assert!(registry
            .drawer_for_field("demo::Host", "y_field", Some("demo::Y"))
            .is_none());
    }

    #[test]
    fn later_registration_overrides_earlier() {
        let mut registry = PropertyDrawerRegistry::new();
        registry.register("demo::X", "f", PropertyDrawer::range(0.0, 1.0));
        registry.register("demo::X", "f", PropertyDrawer::text_area(2, 4));
        assert_eq!(
            registry.drawer_for("demo::X", "f").map(|d| d.renderer),
            Some(PropertyRenderer::TextArea)
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn builtin_declarations_cover_key_fields() {
        let registry = PropertyDrawerRegistry::with_builtin();
        assert!(!registry.is_empty());

        let camera = "prism_engine::scene::Camera";
        let exposure = registry.drawer_for(camera, "exposure").expect("exposure");
        assert_eq!(exposure.renderer, PropertyRenderer::Slider);
        assert_eq!(exposure.range_bounds(), Some((0.0, 5.0)));

        let background = registry
            .drawer_for(camera, "background_color")
            .expect("background_color");
        assert_eq!(background.renderer, PropertyRenderer::Color);
        assert_eq!(background.color_usage_flags(), Some((true, false)));

        let text = registry
            .drawer_for("prism_engine::ui::Text", "content")
            .expect("Text.content");
        assert_eq!(text.renderer, PropertyRenderer::TextArea);
        assert_eq!(text.line_range(), Some((2, 6)));

        assert!(registry
            .drawer_for("prism_engine::scene::Light", "shadow_strength")
            .is_some());
        // Undeclared field has no drawer.
        assert!(registry.drawer_for(camera, "aspect").is_none());
    }

    #[test]
    fn serializes_to_tagged_camel_case_objects() {
        let drawer = PropertyDrawer::range(1.0, 179.0).with_tooltip("fov");
        let json = serde_json::to_value(&drawer).expect("serialize");
        assert_eq!(json["renderer"], "slider");
        assert_eq!(json["attributes"][0]["type"], "range");
        assert_eq!(json["attributes"][0]["min"], 1.0);
        assert_eq!(json["attributes"][0]["max"], 179.0);
        assert_eq!(json["attributes"][1]["type"], "tooltip");

        let text_area = serde_json::to_value(PropertyDrawer::text_area(2, 6)).unwrap();
        assert_eq!(text_area["attributes"][0]["type"], "textArea");
        assert_eq!(text_area["attributes"][0]["minLines"], 2);
        assert_eq!(text_area["attributes"][0]["maxLines"], 6);
    }
}
