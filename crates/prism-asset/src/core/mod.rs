//! # prism-asset::core
//!
//! Prisma resource pipeline basic types
//!
//! Provides primitive building blocks shared by all other crates in pipeline:
//! [`AssetId`], [`AssetGuid`], [`AssetType`], [`Handle<T>`],
//! [`AssetRef`], and ScriptableObject-style [`AssetData`](asset_data::AssetData)
//! trait and [`AssetHandle<T>`](asset_data::AssetHandle).

#[cfg(feature = "asset-data")]
pub mod asset_data;
pub mod asset_type;
pub mod guid;
pub mod handle;
pub mod id;
pub mod meta;

#[cfg(feature = "asset-data")]
pub use asset_data::{AssetData, AssetHandle, LoadedAsset};
pub use asset_type::AssetRef;
pub use asset_type::AssetType;
pub use guid::AssetGuid;
pub use handle::{AnyAsset, AnyHandle, Handle};
pub use id::AssetId;
pub use meta::{AssetMeta, GuidIndex, META_SUFFIX};
