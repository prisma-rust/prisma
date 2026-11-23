//! Build-time asset pipeline: importer → cooker → database.
//!
//! This crate only used in build machine/editor, **does not enter** shipping runtime.
//! Runtime format (`AssetId`/`Handle`/`.pak`/`ResourceManager`) in `prism-asset`.
//!
//! Layers:
//! - `importer`: source file → import-time data (glTF, scene JSON, texture slices…)
//! - `cooker`: import-time data → runtime blob (mesh/texture/material/…)
//! - `db`: asset records and import cache (build-time database)
//! - `pack`: already cooked directory → `.pak` + path manifest (bottom-layer packaging tool)

pub mod aseprite;
pub mod atlas;
pub mod cooker;
pub mod db;
mod fs_atomic;
pub mod import_settings;
pub mod importer;
pub mod pack;
pub mod tiled;

pub use aseprite::{
    cook_aseprite, AsepriteCookResult, AsepriteDocument, AsepriteFrame, AsepriteJsonExport,
};
pub use atlas::{
    encode_rtex_rgba8, pack_atlas, trim_transparent_pixels, AtlasPackOptions, PackedAtlasResult,
    RawSpriteInput,
};
pub use cooker::{
    default_cooker_registry, CookContext, CookError, CookPipeline, CookResult, Cooker,
    CookerRegistry,
};
pub use db::{AssetDatabase, AssetRecord, ImportCache};
pub use import_settings::ImportSettings;
pub use importer::{default_importer_registry, ImportError, ImportPipeline, Importer};
pub use pack::{
    pack_directory, pack_directory_split, pack_directory_with_options, OptionalPackSummary,
    PackOptions, PackSummary, SplitPackSummary, OPTIONAL_PAK_DIR,
};
pub use tiled::{
    cook_tiled_map, encode_rtilemap_bytes, merge_tile_collision_boxes, TiledMap, TiledObjectLayer,
    TiledTileLayer,
};
