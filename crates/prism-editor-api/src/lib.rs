//! Prisma editor / MCP **backend data layer**.
//!
//! This layer deliberately stays pure: only depends on `prism-scene` / `prism-reflect`,
//! **does not depend on egui, Vulkan / engine / rendering**. It exposes "scene schema" and
//! "`.scene.json` document IO" as serializable data structures, for MCP tools
//! (see `crates/prism-editor-mcp`) to convert directly to JSON.
//!
//! Responsibilities:
//! - [`schema`]: projects `prism-scene` component registry into frontend-consumable property panel
//!   schema (field name / type / read-only / nested structure / default value).
//! - [`scene`]: locates project root, enumerates / reads / writes / validates `.scene.json`, scene templates and file ops.
//! - [`presets`]: quick-add entity presets (camera / light / UI / mesh / empty).
//! - [`assets`]: `assets/` directory enumeration (with size / classification) and `.scene.json` reference analysis.
//! - [`reflect_edit`]: **reflection-based** field-level writes (read-only rejected, type validation, author JSON round-trip).
//!
//! Data layer (consumed by `editor/` and automation tools):
//! - [`asset_inspect`]: inspection and validation of cooked assets (RSKL / RSKN / RANI / RMES ...).
//! - [`asset_importer`]: map by extension to importer category / main asset type.
//! - [`property_drawer`]: field render declarations (range / multiline text / color usage / custom draw).
//! - [`scene_manifest`]: `assets/scenes.json` manifest read/write.
//! - [`settings`]: project / render settings model for the engine settings file.
//!

pub mod asset_importer;
pub mod asset_inspect;
pub mod asset_ops;
pub mod assets;
pub mod ini;
pub mod presets;
pub mod property_drawer;
pub mod reflect_edit;
pub mod scene;
pub mod scene_manifest;
pub mod schema;
pub mod settings;

pub use asset_importer::{
    get_at_path as asset_importer_at_path, list_importers, AssetImporter, ImporterClass,
};
pub use asset_ops::{
    asset_reference_documents, create_asset_dir, delete_asset, move_asset, normalize_asset_path,
    rename_asset, AssetMutation,
};
pub use assets::{
    asset_kind, asset_usage, assets_dir, list_asset_dirs, list_assets, list_assets_info,
    read_asset_bytes, read_asset_text, AssetInfo, AssetRef, AssetTextPreview, MAX_ASSETS,
    MAX_PREVIEW_BYTES, MAX_TEXT_PREVIEW_BYTES,
};
pub use presets::{entity_presets, EntityPreset};
pub use property_drawer::{
    PropertyAttribute, PropertyDrawer, PropertyDrawerRegistry, PropertyRenderer,
};
pub use reflect_edit::{normalize_component, set_component_field};
pub use scene::{
    delete_doc, delete_entities, delete_scene, doc_path, docs_dir, empty_scene_doc,
    extract_subtree, find_project_root, instantiate_prefab, list_docs, list_scenes, move_entity,
    prefab_instances, prefab_template_doc, read_doc, read_scene_doc, rename_doc, rename_scene,
    reparent_doc, revert_prefab_instance, scene_path, scenes_dir, sync_prefab_instances,
    template_doc, template_scene_doc, templates, templates_for, unlink_prefab_instance,
    validate_doc, validate_scene_doc, write_doc, write_doc_as, write_doc_in_dir, write_scene_as,
    write_scene_doc, DocKind, InstantiateResult, MoveResult, PrefabLink, SceneEntry, SceneTemplate,
    PREFAB_LINK_COMPONENT,
};
pub use schema::{
    build_schema, build_schema_with, builtin_registry, FieldSchema, Schema, TypeSchema,
};
pub use settings::{
    read_project_settings, read_render_settings, settings_path, write_project_settings,
    write_project_settings_diff, write_render_settings, write_render_settings_diff,
    AppSettingsData, BuildSettingsData, ProjectSettingsData, RenderSettingsData,
    WindowSettingsData,
};
