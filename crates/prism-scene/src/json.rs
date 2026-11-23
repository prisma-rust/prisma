//! Authoring-side `.scene.json` shape.
//!
//! This is the *source* format used by the editor and importers. It is not the
//! shipped runtime format — see [`crate::rscn`] for the compact cooked form.
//!
//! The same types are used by `prism-cooker` (JSON → RSCN) and by the engine's
//! development loader (JSON → ECS), so there is exactly one definition of the
//! scene source schema.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root of a `.scene.json` file.
///
/// When reading use [`SceneJson::parse`] (it first migrates old files to
/// [`crate::migrate::SCENE_DOC_VERSION`]); direct `serde_json::from_str` should only
/// be used for input already at current version.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneJson {
    /// Author format version. Missing treated as 1 (historical files lack this field).
    #[serde(default)]
    pub version: u32,
    pub entities: Vec<EntityJson>,
}

/// One authoring entity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityJson {
    /// Stable entity id: unique within document, not reused after deletion. `parent` references it.
    ///
    /// No `serde` default — input missing `id` must go through
    /// [`crate::migrate::migrate_scene_doc`] (v2→v3 filled by array order),
    /// directly deserializing v2 doc should error, not silently get a heap of `id = 0`.
    pub id: u32,
    #[serde(default)]
    pub name: Option<String>,
    /// Parent entity `id` (**not** array index).
    #[serde(default)]
    pub parent: Option<u32>,
    #[serde(default)]
    pub transform: TransformJson,
    /// Component type name (scene-canonical, e.g.
    /// `prism_engine::scene::Camera`) → serde JSON payload.
    #[serde(default)]
    pub components: HashMap<String, serde_json::Value>,
}

/// Component that is **struct field or runtime-system derived**, therefore not allowed in
/// [`EntityJson::components`].
///
/// - `LocalTransform`: baked by loader / cooker from [`EntityJson::transform`].
///   Author side only has this one "non-removable Transform" (matching engine author state),
///   `LocalTransform` itself belongs to runtime only. Allowing both paths at once creates silent bug:
///   Both loaders "insert `LocalTransform` first + compute `LocalToWorld`,
///   then apply `components`", so component payload overwrites `LocalTransform`
///   while leaving `LocalToWorld` computed from old value in place.
/// - `ComputedLayout`: UI layout system **output** (`rect` overwritten each frame).
///   Flag UI elements use only `Node` / `Style`; writing into document only creates a
///   false field always overwritten in checker, and keeps implicit contract "missing = not rendered".
pub const DERIVED_COMPONENTS: &[&str] = &[
    "prism_engine::scene::LocalTransform",
    "prism_engine::scene::TransformInterpolation2D",
    "prism_engine::scene::ActiveInHierarchy",
    "prism_engine::scene::Persistent",
    "prism_engine::ui::ComputedLayout",
];

/// Author-format local transform.
///
/// Field names match data-oriented `LocalTransform` (`Position` / `Rotation` / `Scale`),
/// but it is **struct field**, not component — see [`DERIVED_COMPONENTS`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformJson {
    #[serde(default)]
    pub position: [f32; 3],
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    #[serde(default = "one_vec3")]
    pub scale: [f32; 3],
}

impl Default for TransformJson {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: identity_quat(),
            scale: one_vec3(),
        }
    }
}

fn identity_quat() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn one_vec3() -> [f32; 3] {
    [1.0; 3]
}

impl SceneJson {
    /// Parse `.scene.json` text, **first migrate** to current version then deserialize.
    ///
    /// All paths reading `.scene.json` / `.prefab.json` should go through here,
    /// to transparently upgrade old files (missing `version`, using `transform.translation`).
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("invalid scene JSON: {e}"))?;
        Self::from_value(value)
    }

    /// Same as [`Self::parse`], but input already a [`serde_json::Value`].
    pub fn from_value(mut value: serde_json::Value) -> Result<Self, String> {
        crate::migrate::migrate_scene_doc(&mut value)?;
        let scene: Self =
            serde_json::from_value(value).map_err(|e| format!("invalid scene document: {e}"))?;
        scene.validate_authorship()?;
        Ok(scene)
    }

    /// Reject author writing derived components directly (see [`DERIVED_COMPONENTS`]).
    fn validate_authorship(&self) -> Result<(), String> {
        for (index, entity) in self.entities.iter().enumerate() {
            for name in DERIVED_COMPONENTS {
                if entity.components.contains_key(*name) {
                    return Err(format!(
                        "entity {index}: component '{name}' is derived from the structural \
                         `transform` field; author `transform` instead"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Next available entity id (`max(id) + 1`; empty document 0).
    ///
    /// id monotonic and **not reused**: holes left by deletion stay, avoiding old reference "resurrecting" onto new entity.
    pub fn next_id(&self) -> u32 {
        self.entities
            .iter()
            .map(|entity| entity.id)
            .max()
            .map_or(0, |max| max.saturating_add(1))
    }

    /// Entity id → array index (for parsing `parent` references).
    pub fn index_of_id(&self) -> HashMap<u32, usize> {
        self.entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.id, index))
            .collect()
    }

    /// Validate structural invariants that the binary format relies on.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_authorship()?;
        if self.entities.is_empty() {
            return Err("scene must contain at least one entity".into());
        }
        let mut ids = std::collections::HashSet::with_capacity(self.entities.len());
        for (index, entity) in self.entities.iter().enumerate() {
            if !ids.insert(entity.id) {
                return Err(format!("entity {index}: duplicate id {}", entity.id));
            }
        }
        for (index, entity) in self.entities.iter().enumerate() {
            if let Some(parent) = entity.parent {
                if parent == entity.id {
                    return Err(format!("entity {index}: self-parent not allowed"));
                }
                if !ids.contains(&parent) {
                    return Err(format!("entity {index}: parent id {parent} does not exist"));
                }
            }
        }
        Ok(())
    }

    /// Return entity indices parent-first.
    ///
    /// Cycles (which [`Self::validate`] only rejects for self-parents) are
    /// broken deterministically by appending the remaining entities.
    ///
    /// Linear in the number of entities: ids are resolved once and children
    /// are bucketed once instead of rescanning the array for every node.
    pub fn topological_order(&self) -> Vec<usize> {
        let n = self.entities.len();
        let mut order = Vec::with_capacity(n);
        let mut visited = vec![false; n];

        let index_of_id = self.index_of_id();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for (index, entity) in self.entities.iter().enumerate() {
            match entity
                .parent
                .and_then(|parent| index_of_id.get(&parent).copied())
            {
                Some(parent_index) => children[parent_index].push(index),
                None => roots.push(index),
            }
        }

        let mut queue = roots;
        while let Some(index) = queue.pop() {
            if visited[index] {
                continue;
            }
            visited[index] = true;
            order.push(index);
            queue.extend(children[index].iter().rev().copied());
        }

        for (index, seen) in visited.iter().enumerate() {
            if !seen {
                order.push(index);
            }
        }
        order
    }
}

#[cfg(test)]
#[path = "json_tests.rs"]
mod tests;
