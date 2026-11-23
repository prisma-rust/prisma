//! Modern scene system — ECS components, loading, hierarchy, and systems.
//!
//! Scene load (manifest → world) unifiedly responsible by scene system [`SceneManager`];
//! `Engine::init_scene` delegates to [`SceneManager::load_first_from_manifest`].
//!
//! See `docs/DESIGN.md` (scene/asset layering) and the root `AGENTS.md`
//! ("Scene / asset layering rules").

pub mod component_registry {
    //! Compatibility re-export of [`prism_scene::registry`].
    pub use prism_scene::registry::*;
}
pub mod runtime_components;
pub mod components {
    //! Scene-schema components (from `prism-scene`) plus runtime GPU refs.
    pub use super::runtime_components::{
        Hidden, LodHidden, MaterialRef, MaterialSlots, MeshRef, MeshSubmesh, MeshSubmeshes,
        SceneEntityId, ShaderRef, SpriteRef,
    };
    pub use prism_scene::components::*;
}
pub mod helpers {
    //! Compatibility re-export of [`prism_scene::helpers`].
    pub use prism_scene::helpers::*;
}
pub mod hot_reload;
pub mod loader;
pub mod scene_system;

pub mod systems;

pub use component_registry::{register_builtin_components, ComponentRegistry};
pub use loader::SceneInstance;
pub use scene_system::{
    EnvironmentProvider, SceneManager, SceneManifest, SceneManifestEntry, SceneReadView,
};

/// Scene hierarchy adapter for the editor checker.
///
/// Roots: entities with [`LocalTransform`] or [`Name`] but no [`Parent`].
/// Child: via [`Child`] component.
///
/// External tools / editors can provide read-only view for same structure — engine itself does not depend on editor.
pub struct SceneHierarchy;
