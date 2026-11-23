//! Component schema: project `prism-scene`'s reflection registration table into a shape consumable by frontend property panels.
//!
//! Frontend hardcodes no component type — after getting [`Schema`] it generates corresponding controls by field `kind`
//! (`bool` / `f32` / `string` / `struct` / `list` / `option` / `array`...),
//! and `struct` fields recursively look up table by type name.

use prism_reflect::{FieldDesc, FieldType, TypeRegistry};
use prism_scene::ComponentRegistry;
use serde::Serialize;

use crate::property_drawer::{PropertyDrawer, PropertyDrawerRegistry};

/// Field schema of one type.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldSchema {
    /// Schema field name (author JSON key).
    pub name: String,
    /// Control type tag: `bool` / `i32` / `f32` / `f64` / `string` /
    /// `struct` / `list` / `option` / `array`.
    pub kind: &'static str,
    /// Nested type name when `kind == "struct"` (frontend recursively looks up [`Schema`] by name).
    pub struct_type: Option<String>,
    /// Element schema for `list` / `option` / `array`.
    pub inner: Option<Box<FieldSchema>>,
    /// Fixed array length for `array`.
    pub array_len: Option<usize>,
    /// Enum variant names (in declaration order). When `Some`, frontend should render a **dropdown**,
    /// option text and written-back value are both variant name (i.e. string in author JSON).
    pub variants: Option<Vec<String>>,
    /// Editor read-only (`#[reflect(read_only)]`).
    pub read_only: bool,
    /// Field drawer = `PropertyDrawer` (`[Range]` / `[TextArea]` /
    /// `[ColorUsage]` / `[CustomPropertyDrawer]`). When absent (not serialized) frontend picks default control
    /// by `kind`. See [`crate::property_drawer`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drawer: Option<PropertyDrawer>,
}

/// Schema of one type (component or nested struct).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TypeSchema {
    /// Normalized type name (like `prism_engine::scene::Camera`).
    pub name: String,
    /// Stable type id (16-bit hex).
    pub type_id: String,
    /// Schema version.
    pub version: u32,
    /// Whether it is a component attachable to entities (`false` = nested field type only).
    pub is_component: bool,
    /// Field list (source order).
    pub fields: Vec<FieldSchema>,
    /// Default author JSON when "adding" component (`None` for non-components).
    pub default_json: Option<serde_json::Value>,
}

/// Type table for whole editor.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Schema {
    /// Components first, nested structs after, each sorted by type name.
    pub types: Vec<TypeSchema>,
}

impl Schema {
    /// Look up one entry by type name.
    pub fn type_by_name(&self, name: &str) -> Option<&TypeSchema> {
        self.types.iter().find(|ty| ty.name == name)
    }

    /// Type names of all attachable components (stable order).
    pub fn component_names(&self) -> Vec<&str> {
        self.types
            .iter()
            .filter(|ty| ty.is_component)
            .map(|ty| ty.name.as_str())
            .collect()
    }
}

/// Process-wide shared built-in component registry.
///
/// Registering built-in components requires building several `HashMap`s and registering whole reflection type tree, while field write /
/// normalization is an **input-event-level** hot path (called every frame when dragging slider), so here
/// build only once. Instance provided by `prism_scene::builtin_registry`; editor, engine and
/// project modules share same schema. Registry read-only after build, callbacks all require
/// `Send + Sync`, can be safely shared across threads.
///
/// Host needing to append custom components still uses [`build_schema_with`] to pass its own registry,
/// not affected by this.
pub fn builtin_registry() -> &'static ComponentRegistry {
    prism_scene::builtin_registry()
}

/// Build complete schema (only source of built-in components is `prism-scene`'s registry).
///
/// Field drawers use built-in declarations from [`PropertyDrawerRegistry::with_builtin`].
pub fn build_schema() -> Schema {
    build_schema_from(builtin_registry())
}

/// Build schema with given registry + given drawer registry.
///
/// Host (non-plugin, custom components) can append its own
/// `[CustomPropertyDrawer]` on top of built-in declarations; field-level declarations take priority over type-level.
pub fn build_schema_with(registry: &ComponentRegistry, drawers: &PropertyDrawerRegistry) -> Schema {
    let component_names = registry.component_names();
    let types_registry = registry.type_registry();

    let mut types: Vec<TypeSchema> = types_registry
        .iter()
        .map(|(_, entry)| {
            let desc = entry.descriptor();
            let is_component = component_names.contains(&desc.name);
            TypeSchema {
                name: desc.name.to_string(),
                type_id: desc.id.to_string(),
                version: desc.version,
                is_component,
                fields: desc
                    .fields
                    .iter()
                    .map(|field| field_schema(field, types_registry, desc.name, drawers))
                    .collect(),
                default_json: if is_component {
                    registry.default_json(desc.name)
                } else {
                    None
                },
            }
        })
        .collect();

    types.sort_by(|a, b| {
        b.is_component
            .cmp(&a.is_component)
            .then_with(|| a.name.cmp(&b.name))
    });

    Schema { types }
}

/// Build schema with given registry, drawers use built-in declarations. Tests can inject a component containing only container enum
/// fields this way, without adding a real component into `prism-scene` just to cover schema logic.
fn build_schema_from(registry: &ComponentRegistry) -> Schema {
    build_schema_with(registry, &PropertyDrawerRegistry::with_builtin())
}

fn field_schema(
    field: &FieldDesc,
    registry: &TypeRegistry,
    declaring_type: &str,
    drawers: &PropertyDrawerRegistry,
) -> FieldSchema {
    let mut schema = leaf_schema(&field.ty, registry);
    schema.name = field.name.to_string();
    schema.read_only = field.read_only;
    let value_type = value_type_name(&schema);
    schema.drawer = drawers
        .drawer_for_field(declaring_type, field.name, value_type)
        .cloned();
    if let Some(variants) = field.enum_variants {
        attach_variants(
            &mut schema,
            variants.iter().map(|name| (*name).to_string()).collect(),
        );
    }
    schema
}

fn value_type_name(schema: &FieldSchema) -> Option<&str> {
    if let Some(ref st) = schema.struct_type {
        return Some(st.as_str());
    }
    if let Some(ref inner) = schema.inner {
        return value_type_name(inner);
    }
    None
}

/// Attach enum variants to the **innermost layer** scalar leaf.
///
/// `enum_variants()` forwards inward through `Vec<T>` / `Option<T>` / `[T; N]`, so
/// a `Vec<Enum>` field gets variant names that `inner` (`u8`) should have; frontend only has the leaf control
/// (and array / list elements' own `FieldEditor`) will render dropdown.
fn attach_variants(schema: &mut FieldSchema, variants: Vec<String>) {
    match schema.inner.as_mut() {
        Some(inner) if matches!(schema.kind, "list" | "option" | "array") => {
            attach_variants(inner, variants);
        }
        _ => schema.variants = Some(variants),
    }
}

/// Generate nameless field schema (reused for list/option/array elements and top level).
fn leaf_schema(ty: &FieldType, registry: &TypeRegistry) -> FieldSchema {
    let (kind, struct_type, inner, array_len) = match ty {
        FieldType::Bool => ("bool", None, None, None),
        FieldType::I8 => ("i8", None, None, None),
        FieldType::I16 => ("i16", None, None, None),
        FieldType::I32 => ("i32", None, None, None),
        FieldType::I64 => ("i64", None, None, None),
        FieldType::U8 => ("u8", None, None, None),
        FieldType::U16 => ("u16", None, None, None),
        FieldType::U32 => ("u32", None, None, None),
        FieldType::U64 => ("u64", None, None, None),
        FieldType::F32 => ("f32", None, None, None),
        FieldType::F64 => ("f64", None, None, None),
        FieldType::String => ("string", None, None, None),
        FieldType::Struct(id) => (
            "struct",
            registry.descriptor(*id).map(|desc| desc.name.to_string()),
            None,
            None,
        ),
        FieldType::List(inner) => (
            "list",
            None,
            Some(Box::new(leaf_schema(inner, registry))),
            None,
        ),
        FieldType::Optional(inner) => (
            "option",
            None,
            Some(Box::new(leaf_schema(inner, registry))),
            None,
        ),
        FieldType::Array(inner, len) => (
            "array",
            None,
            Some(Box::new(leaf_schema(inner, registry))),
            Some(*len),
        ),
    };

    FieldSchema {
        name: String::new(),
        kind,
        struct_type,
        inner,
        array_len,
        variants: None,
        read_only: false,
        drawer: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_scene::register_builtin_components;

    #[test]
    fn schema_exposes_enum_variants_for_dropdowns() {
        let schema = build_schema();

        let camera = schema
            .type_by_name("prism_engine::scene::Camera")
            .expect("Camera schema");
        let clear_flags = camera
            .fields
            .iter()
            .find(|f| f.name == "clear_flags")
            .expect("Camera.clear_flags");
        assert_eq!(clear_flags.kind, "u8");
        assert_eq!(
            clear_flags.variants.as_deref(),
            Some(
                &[
                    "Skybox".to_string(),
                    "SolidColor".to_string(),
                    "Depth".to_string(),
                    "Nothing".to_string()
                ][..]
            )
        );

        let text = schema
            .type_by_name("prism_engine::ui::Text")
            .expect("Text schema");
        let alignment = text
            .fields
            .iter()
            .find(|f| f.name == "alignment")
            .expect("Text.alignment");
        assert_eq!(
            alignment.variants.as_deref(),
            Some(
                &[
                    "Center".to_string(),
                    "Left".to_string(),
                    "Right".to_string()
                ][..]
            )
        );

        let exposure = camera.fields.iter().find(|f| f.name == "exposure").unwrap();
        assert!(exposure.variants.is_none());
    }

    #[test]
    fn schema_exposes_components_and_nested_types() {
        let schema = build_schema();

        let first_non_component = schema
            .types
            .iter()
            .position(|ty| !ty.is_component)
            .unwrap_or(schema.types.len());
        assert!(schema.types[..first_non_component]
            .iter()
            .all(|ty| ty.is_component));
        assert!(schema.types[first_non_component..]
            .iter()
            .all(|ty| !ty.is_component));

        let camera = schema
            .type_by_name("prism_engine::scene::Camera")
            .expect("Camera schema");
        assert!(camera.is_component);
        assert!(camera.default_json.is_some());

        assert!(
            camera
                .fields
                .iter()
                .find(|f| f.name == "aspect")
                .unwrap()
                .read_only
        );
        assert!(
            !camera
                .fields
                .iter()
                .find(|f| f.name == "enabled")
                .unwrap()
                .read_only
        );

        assert!(schema.type_by_name("glam::Vec3").is_some());
        assert!(schema.type_by_name("prism_engine::ui::Anchors").is_some());

        let style = schema.type_by_name("prism_engine::ui::Style").unwrap();
        let anchors = style.fields.iter().find(|f| f.name == "anchors").unwrap();
        assert_eq!(anchors.kind, "struct");
        assert_eq!(
            anchors.struct_type.as_deref(),
            Some("prism_engine::ui::Anchors")
        );
        let width = style.fields.iter().find(|f| f.name == "width").unwrap();
        assert_eq!(width.kind, "option");
        assert_eq!(width.inner.as_ref().unwrap().kind, "f32");

        let vec3 = schema.type_by_name("glam::Vec3").unwrap();
        assert!(!vec3.is_component);
        assert!(vec3.default_json.is_none());
        assert_eq!(vec3.fields.len(), 3);
    }

    #[test]
    fn array_fields_report_length_and_element_kind() {
        let schema = build_schema();
        let layout = schema
            .type_by_name("prism_engine::ui::ComputedLayout")
            .unwrap();
        let rect = layout.fields.iter().find(|f| f.name == "rect").unwrap();
        assert_eq!(rect.kind, "array");
        assert_eq!(rect.array_len, Some(4));
        assert_eq!(rect.inner.as_ref().unwrap().kind, "f32");
    }

    #[test]
    fn builtin_drawers_reach_editor_fields() {
        use crate::property_drawer::PropertyRenderer;

        let schema = build_schema();
        let field = |type_name: &str, field: &str| {
            schema
                .type_by_name(type_name)
                .unwrap_or_else(|| panic!("missing type {type_name}"))
                .fields
                .iter()
                .find(|f| f.name == field)
                .unwrap_or_else(|| panic!("missing field {type_name}.{field}"))
                .drawer
                .clone()
        };

        let exposure = field("prism_engine::scene::Camera", "exposure").expect("Camera.exposure");
        assert_eq!(exposure.renderer, PropertyRenderer::Slider);
        assert_eq!(exposure.range_bounds(), Some((0.0, 5.0)));
        assert!(exposure.tooltip().is_some());

        let background =
            field("prism_engine::scene::Camera", "background_color").expect("background_color");
        assert_eq!(background.renderer, PropertyRenderer::Color);
        assert_eq!(background.color_usage_flags(), Some((true, false)));

        let content = field("prism_engine::ui::Text", "content").expect("Text.content");
        assert_eq!(content.renderer, PropertyRenderer::TextArea);
        assert_eq!(content.line_range(), Some((2, 6)));

        assert!(field("prism_engine::scene::Camera", "aspect").is_none());
    }

    #[test]
    fn builtin_registry_is_shared_and_matches_a_fresh_one() {
        assert!(std::ptr::eq(builtin_registry(), builtin_registry()));

        let mut fresh = ComponentRegistry::new();
        register_builtin_components(&mut fresh);
        assert_eq!(
            builtin_registry().component_names(),
            fresh.component_names()
        );

        assert_eq!(
            build_schema().component_names(),
            builtin_registry().component_names()
        );
    }

    #[test]
    fn custom_drawers_can_be_added_on_top_of_builtins() {
        use crate::property_drawer::{PropertyDrawer, PropertyDrawerRegistry, PropertyRenderer};

        let mut drawers = PropertyDrawerRegistry::with_builtin();
        drawers.register_custom(
            "prism_engine::scene::Camera",
            "background_color",
            "LinearColorEditor",
        );
        drawers.register(
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            PropertyDrawer::range(10.0, 120.0),
        );

        let mut registry = ComponentRegistry::new();
        register_builtin_components(&mut registry);
        let custom_schema = build_schema_with(&registry, &drawers);

        let camera = custom_schema
            .type_by_name("prism_engine::scene::Camera")
            .unwrap();
        let background = camera
            .fields
            .iter()
            .find(|f| f.name == "background_color")
            .unwrap()
            .drawer
            .clone()
            .expect("custom drawer");
        assert_eq!(background.renderer, PropertyRenderer::Custom);
        assert_eq!(background.custom.as_deref(), Some("LinearColorEditor"));

        let fov = camera
            .fields
            .iter()
            .find(|f| f.name == "fov_y_degrees")
            .unwrap()
            .drawer
            .clone()
            .expect("fov drawer");
        assert_eq!(fov.range_bounds(), Some((10.0, 120.0)));

        let exposure = camera
            .fields
            .iter()
            .find(|f| f.name == "exposure")
            .unwrap()
            .drawer
            .clone()
            .expect("exposure drawer");
        assert_eq!(exposure.range_bounds(), Some((0.0, 5.0)));

        drawers.register_type(
            "prism_engine::ui::Anchors",
            PropertyDrawer::custom("AnchorGizmo"),
        );
        let custom_schema_with_type = build_schema_with(&registry, &drawers);
        let style = custom_schema_with_type
            .type_by_name("prism_engine::ui::Style")
            .expect("Style component");
        let anchors_field = style
            .fields
            .iter()
            .find(|f| f.name == "anchors")
            .expect("Style.anchors");
        let anchors_drawer = anchors_field
            .drawer
            .as_ref()
            .expect("anchors type-level drawer");
        assert_eq!(anchors_drawer.renderer, PropertyRenderer::Custom);
        assert_eq!(anchors_drawer.custom.as_deref(), Some("AnchorGizmo"));

        let builtin_schema = build_schema();
        let builtin_background = builtin_schema
            .type_by_name("prism_engine::scene::Camera")
            .and_then(|ty| ty.fields.iter().find(|f| f.name == "background_color"))
            .and_then(|f| f.drawer.as_ref())
            .expect("builtin color drawer");
        assert_eq!(builtin_background.renderer, PropertyRenderer::Color);
    }

    use prism_reflect::{FieldType, Reflect, ReflectError, ReflectType, Value};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    enum TestAlign {
        #[default]
        Left,
        Center,
    }

    impl ReflectType for TestAlign {
        fn field_type() -> FieldType {
            FieldType::U8
        }

        fn enum_variants() -> Option<&'static [&'static str]> {
            Some(&["Left", "Center"])
        }

        fn to_value(&self) -> Value {
            Value::U64(u64::from(*self as u8))
        }

        fn from_value(value: Value) -> Result<Self, ReflectError> {
            match value.as_u64()? {
                0 => Ok(Self::Left),
                1 => Ok(Self::Center),
                other => Err(ReflectError::BadData(format!("invalid TestAlign: {other}"))),
            }
        }
    }

    #[derive(Debug, Default, Reflect)]
    #[reflect(name = "test::ContainerEnums")]
    struct ContainerEnums {
        flags: Vec<TestAlign>,
        maybe: Option<TestAlign>,
        fixed: [TestAlign; 2],
    }

    #[test]
    fn container_enum_variants_reach_the_inner_leaf() {
        let mut registry = ComponentRegistry::new();
        registry.register_reflect::<ContainerEnums>();
        let schema = build_schema_from(&registry);
        let ty = schema
            .type_by_name("test::ContainerEnums")
            .expect("test component schema");
        let field = |name: &str| ty.fields.iter().find(|f| f.name == name).unwrap();
        let expected = || Some(vec!["Left".to_string(), "Center".to_string()]);

        for (name, kind) in [("flags", "list"), ("maybe", "option"), ("fixed", "array")] {
            let schema = field(name);
            assert_eq!(schema.kind, kind, "{name} kind");
            assert_eq!(
                schema.variants, None,
                "{name} must not carry variants itself"
            );
            let inner = schema.inner.as_ref().expect("container inner schema");
            assert_eq!(inner.kind, "u8", "{name} inner kind");
            assert_eq!(inner.variants, expected(), "{name} inner variants");
        }

        assert_eq!(field("fixed").array_len, Some(2));
    }
}
