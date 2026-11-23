//! Entity presets: prebuilt entities for toolbar "quick add" (camera / light / UI / mesh / empty).
//!
//! Presets only declare **which components to attach** (canonical names from registry) and initial position; field values from
//! frontend via `prism-editor-api` schema (`default_json`), keeping single source.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// One entity preset.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EntityPreset {
    /// Stable id (frontend for key / icon).
    pub id: String,
    /// Display name.
    pub label: String,
    /// New entity `name` initial value.
    pub name: String,
    /// Canonical names of components to attach (order is attachment order).
    pub components: Vec<String>,
    /// Component field overrides (canonical name → partial JSON), frontend shallow-merges with schema defaults.
    ///
    /// For presets of "directional / spot" class **same component different config** —
    /// component name alone only gets `Light` default (point light).
    #[serde(default)]
    pub overrides: BTreeMap<String, Value>,
    /// Initial rotation (quaternion xyzw).
    ///
    /// Light direction determined by `transform.forward` (+Z), so directional / spot must carry rotation.
    pub rotation: [f32; 4],
    /// Initial offset.
    pub position: [f32; 3],
}

/// Built-in entity presets (order is UI display order).
pub fn entity_presets() -> Vec<EntityPreset> {
    let preset =
        |id: &str, label: &str, name: &str, components: &[&str], position: [f32; 3]| EntityPreset {
            id: id.to_string(),
            label: label.to_string(),
            name: name.to_string(),
            components: components.iter().map(|c| c.to_string()).collect(),
            overrides: BTreeMap::new(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            position,
        };

    let sun = |mut p: EntityPreset| {
        p.components = vec!["prism_engine::scene::Light".to_string()];
        p.overrides.insert(
            "prism_engine::scene::Light".to_string(),
            json!({
                "light_type": "Directional",
                "intensity": 100_000.0,
                "shadows": "Soft",
            }),
        );
        let sqrt3 = 3.0_f32.sqrt();
        p.rotation = [
            -std::f32::consts::FRAC_1_SQRT_2 / sqrt3,
            -0.5 / sqrt3,
            0.0,
            sqrt3 * 0.5,
        ];
        p
    };

    let spot = |mut p: EntityPreset| {
        p.components = vec!["prism_engine::scene::Light".to_string()];
        p.overrides.insert(
            "prism_engine::scene::Light".to_string(),
            json!({
                "light_type": "Spot",
                "intensity": 200.0,
                "range": 20.0,
                "spot_angle": 45.0,
                "inner_spot_angle": 30.0,
                "shadows": "Soft",
            }),
        );
        p.rotation = [
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        p
    };

    let primitive = |id: &str, label: &str, name: &str, mesh: &str, position: [f32; 3]| {
        let mut p = preset(
            id,
            label,
            name,
            &[
                "prism_engine::scene::MeshRenderer",
                "prism_engine::scene::Active",
            ],
            position,
        );
        p.overrides.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            json!({ "mesh_path": format!("__builtin/{mesh}") }),
        );
        p
    };

    vec![
        primitive("cube", "Cube", "Cube", "box", [0.0, 0.5, 0.0]),
        primitive("sphere", "Sphere", "Sphere", "sphere", [0.0, 0.5, 0.0]),
        primitive(
            "cylinder",
            "Cylinder",
            "Cylinder",
            "cylinder",
            [0.0, 0.5, 0.0],
        ),
        primitive("capsule", "Capsule", "Capsule", "capsule", [0.0, 1.0, 0.0]),
        primitive("quad", "Quad", "Quad", "quad", [0.0, 0.0, 0.0]),
        primitive("plane", "Plane", "Plane", "quad", [0.0, 0.0, 0.0]),
        preset("empty", "Empty entity", "entity", &[], [0.0, 0.0, 0.0]),
        preset(
            "camera",
            "Camera",
            "Camera",
            &[
                "prism_engine::scene::Camera",
                "prism_engine::scene::FlyCameraController",
            ],
            [0.0, 0.0, 10.0],
        ),
        sun(preset(
            "sun",
            "Directional light",
            "Sun",
            &[],
            [0.0, 6.0, 0.0],
        )),
        preset(
            "point_light",
            "Point light",
            "PointLight",
            &["prism_engine::scene::Light"],
            [3.0, 3.0, 3.0],
        ),
        spot(preset(
            "spot_light",
            "Spot light",
            "SpotLight",
            &[],
            [0.0, 6.0, 0.0],
        )),
        preset(
            "mesh",
            "Mesh",
            "Mesh",
            &[
                "prism_engine::scene::MeshRenderer",
                "prism_engine::scene::Active",
            ],
            [0.0, 0.0, 0.0],
        ),
        preset(
            "skybox",
            "Skybox",
            "Skybox",
            &["prism_engine::scene::Skybox"],
            [0.0, 0.0, 0.0],
        ),
        preset(
            "ui",
            "UI element",
            "ui_node",
            &["prism_engine::ui::Node", "prism_engine::ui::Style"],
            [0.0, 0.0, 0.0],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_only_reference_registered_components() {
        let schema = crate::build_schema();
        let components: Vec<&str> = schema.component_names();

        for preset in entity_presets() {
            assert!(!preset.id.is_empty());
            assert!(!preset.label.is_empty());
            assert!(!preset.name.is_empty());
            for component in &preset.components {
                assert!(
                    components.contains(&component.as_str()),
                    "preset '{}' references unregistered component '{component}'",
                    preset.id
                );
            }
        }

        let camera = entity_presets()
            .into_iter()
            .find(|p| p.id == "camera")
            .expect("camera preset");
        assert!(camera
            .components
            .contains(&"prism_engine::scene::Camera".to_string()));
    }
}

#[cfg(test)]
mod light_preset_tests {
    use super::*;

    fn preset(id: &str) -> EntityPreset {
        entity_presets()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("preset '{id}'"))
    }

    #[test]
    fn preset_rotations_are_unit_quaternions() {
        for preset in entity_presets() {
            let [x, y, z, w] = preset.rotation;
            let len = (x * x + y * y + z * z + w * w).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "Preset '{}' rotation is not identity quaternion: {len}",
                preset.id
            );
        }
    }

    #[test]
    fn light_presets_use_the_light_component() {
        for id in ["sun", "point_light", "spot_light"] {
            let preset = preset(id);
            assert_eq!(
                preset.components,
                vec!["prism_engine::scene::Light".to_string()],
                "Preset '{id}' should use unified Light component"
            );
        }

        let light_type = |id: &str| {
            preset(id)
                .overrides
                .get("prism_engine::scene::Light")
                .and_then(|value| value.get("light_type"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        assert_eq!(light_type("sun").as_deref(), Some("Directional"));
        assert_eq!(light_type("spot_light").as_deref(), Some("Spot"));
        assert_eq!(light_type("point_light"), None);
    }

    #[test]
    fn preset_overrides_only_target_existing_fields() {
        let schema = crate::build_schema();

        for preset in entity_presets() {
            for (component, overrides) in &preset.overrides {
                assert!(
                    preset.components.contains(component),
                    "Preset '{}' overrides unattached component '{component}'",
                    preset.id
                );
                let ty = schema
                    .type_by_name(component)
                    .unwrap_or_else(|| panic!("No component '{component}' in schema"));
                let fields = overrides
                    .as_object()
                    .unwrap_or_else(|| panic!("Preset '{}' overrides not object", preset.id));
                for key in fields.keys() {
                    assert!(
                        ty.fields.iter().any(|field| &field.name == key),
                        "Preset '{}' overrides nonexistent field '{key}' on '{component}'",
                        preset.id
                    );
                }
            }
        }
    }
}
