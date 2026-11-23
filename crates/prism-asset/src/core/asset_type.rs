//! Asset type classification.

use serde::{Deserialize, Serialize};

/// High-level classification of resource data format.
///
/// Discriminant stored as `u32` in binary `.pak` format,
/// therefore at runtime no need for string table to dispatch to correct loader.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetType {
    /// Raw binary data block---runtime loads as `Vec<u8>`.
    Binary = 0,
    /// GPU texture/image
    Texture = 1,
    /// Geometric mesh (vertex + index data).
    Mesh = 2,
    /// GPU material parameter block + shader bindings.
    Material = 3,
    /// Shader source or compiled bytecode.
    Shader = 4,
    /// Prefab — a reusable entity template.
    Prefab = 5,
    /// Complete scene graph.
    Scene = 6,
    /// Audio clip (PCM, Ogg, etc.).
    Audio = 7,
    /// Font files (ttf / otf / ttc), packed as-is, handed to UI text rasterizer at runtime (#583).
    Font = 8,
    /// Skeleton hierarchy + reference pose + inverse bind (`.rskel`, #560).
    Skeleton = 9,
    /// Skinned mesh with 4 bone weights + submesh (`.rskin`, #560).
    SkinnedMesh = 10,
    /// Skeletal animation clip (`.ranim`, #560).
    AnimClip = 11,
    /// Animation state machine (`.rasm`, #560 P1).
    StateMachine = 12,
    /// Animation graph (`.rag`, #560 P1).
    AnimGraph = 13,
    /// 2D sprite atlas metadata (`.ratlas`, #1158).
    Atlas = 14,
    /// 2D skeleton/frame animation metadata (`.ranim2d` / Aseprite, #1159).
    Anim2D = 15,
    /// 2D tilemap level data (`.rtilemap` / Tiled, #1159).
    Tilemap = 16,
    /// Fallback for unrecognised formats.
    Unknown = 0xFF,
}

impl AssetType {
    /// Return `true` if this type is known (not `Unknown`).
    pub fn is_known(self) -> bool {
        self != AssetType::Unknown
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            AssetType::Binary => "binary",
            AssetType::Texture => "texture",
            AssetType::Mesh => "mesh",
            AssetType::Material => "material",
            AssetType::Shader => "shader",
            AssetType::Prefab => "prefab",
            AssetType::Scene => "scene",
            AssetType::Audio => "audio",
            AssetType::Font => "font",
            AssetType::Skeleton => "skeleton",
            AssetType::SkinnedMesh => "skinned-mesh",
            AssetType::AnimClip => "anim-clip",
            AssetType::StateMachine => "state-machine",
            AssetType::AnimGraph => "anim-graph",
            AssetType::Atlas => "atlas",
            AssetType::Anim2D => "anim-2d",
            AssetType::Tilemap => "tilemap",
            AssetType::Unknown => "unknown",
        }
    }

    /// Build an `AssetType` from its raw `u32` discriminant.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => AssetType::Binary,
            1 => AssetType::Texture,
            2 => AssetType::Mesh,
            3 => AssetType::Material,
            4 => AssetType::Shader,
            5 => AssetType::Prefab,
            6 => AssetType::Scene,
            7 => AssetType::Audio,
            8 => AssetType::Font,
            9 => AssetType::Skeleton,
            10 => AssetType::SkinnedMesh,
            11 => AssetType::AnimClip,
            12 => AssetType::StateMachine,
            13 => AssetType::AnimGraph,
            14 => AssetType::Atlas,
            15 => AssetType::Anim2D,
            16 => AssetType::Tilemap,
            _ => AssetType::Unknown,
        }
    }

    /// Get the raw `u32` discriminant.
    pub fn to_u32(self) -> u32 {
        self as u32
    }

    /// Infer asset type from a file extension (lowercase, without leading dot).
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "bin" | "bytes" => AssetType::Binary,
            "rscn" => AssetType::Scene,
            "rtex" => AssetType::Texture,
            "rmes" => AssetType::Mesh,
            "rmat" => AssetType::Material,
            "rskel" => AssetType::Skeleton,
            "rskin" => AssetType::SkinnedMesh,
            "ranim" => AssetType::AnimClip,
            "rasm" => AssetType::StateMachine,
            "rag" => AssetType::AnimGraph,
            "ratlas" => AssetType::Atlas,
            "ranim2d" | "ase" | "aseprite" => AssetType::Anim2D,
            "rtilemap" | "tmx" | "tmj" | "tsx" => AssetType::Tilemap,
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "hdr" | "exr" | "ktx" | "ktx2" | "dds" => {
                AssetType::Texture
            }
            "gltf" | "glb" | "fbx" | "obj" | "stl" | "ply" => AssetType::Mesh,
            "mat" | "material" => AssetType::Material,
            "slang" | "hlsl" | "glsl" | "spv" | "wgsl" => AssetType::Shader,
            "prefab" => AssetType::Prefab,
            "scene" => AssetType::Scene,
            "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" => AssetType::Audio,
            "ttf" | "otf" | "ttc" => AssetType::Font,
            _ => AssetType::Unknown,
        }
    }
}

/// A lightweight editor-side reference to another asset.
///
/// Unlike `Handle<T>`, this is serializable and contains only the stable
/// identity, never a runtime slot index. Use this in JSON metadata, importer
/// dependency lists, and prefab asset references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRef {
    pub id: crate::core::AssetId,
    pub asset_type: AssetType,
}

impl AssetRef {
    /// Create a new reference.
    pub fn new(id: crate::core::AssetId, asset_type: AssetType) -> Self {
        Self { id, asset_type }
    }
}

#[cfg(test)]
#[path = "asset_type_tests.rs"]
mod tests;
