//! # prism-asset-types
//!
//! ScriptableObject-style resource type definitions.
//!
//! Every struct in this crate implements `AssetData`,
//! each with a unique `typetag` name, enabling polymorphic serialization ---
//! editor can open *any* resource file as `Box<dyn AssetData>` at compile time without knowing the type ---
//!
//! ## Types
//!
//! | Type | typetag name | Description |
//! |------|-------------|-------------|
//! | [`CubeDef`] | `"cube"` | Cube map texture source |
//! | [`MaterialDef`] | `"material"` | PBR material definition |
//! | [`TextureDef`] | `"texture"` | Texture source reference |

pub mod cube_def;
pub mod material_def;

pub use cube_def::CubeDef;
pub use material_def::{MaterialDef, TextureDef};
