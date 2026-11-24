//! Data-driven 2D sprite atlas slice and animation configuration.
//!
//! Allows game data files (JSON) to declare atlas paths, pixel rectangles, pivots,
//! pixels-per-unit, and animation sequences without hardcoding texture coordinates in gameplay code.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::components::{SpriteAnimation, SpriteRenderer};

fn default_pivot() -> [f32; 2] {
    [0.5, 0.5]
}

fn default_ppu() -> f32 {
    16.0
}

fn default_frames() -> u32 {
    1
}

fn default_looped() -> bool {
    true
}

/// Definition of a single named sprite or horizontal frame strip inside a texture atlas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpriteSliceDef {
    /// Texture path relative to `assets/` (for example `"sprites/atlas.png"`).
    pub texture: String,
    /// Pixel rectangle `[x, y, width, height]` of frame 0 from the top-left corner of the texture.
    pub rect_px: [u32; 4],
    /// Normalized pivot `[x, y]` with `[0.0, 0.0]` at bottom-left and `[0.5, 0.5]` centered.
    #[serde(default = "default_pivot")]
    pub pivot: [f32; 2],
    /// Pixels per world unit (`SpriteRenderer::pixels_per_unit`).
    #[serde(default = "default_ppu")]
    pub pixels_per_unit: f32,
    /// Number of contiguous horizontal frames starting at `rect_px`.
    #[serde(default = "default_frames")]
    pub frames: u32,
    /// Playback speed in frames per second (`0.0` = static frame).
    pub fps: f32,
    /// Whether the animation loops (`true` by default).
    #[serde(default = "default_looped")]
    pub looped: bool,
}

impl Default for SpriteSliceDef {
    fn default() -> Self {
        Self {
            texture: String::new(),
            rect_px: [0, 0, 0, 0],
            pivot: default_pivot(),
            pixels_per_unit: default_ppu(),
            frames: default_frames(),
            fps: 0.0,
            looped: default_looped(),
        }
    }
}

impl SpriteSliceDef {
    /// Populate a [`SpriteRenderer`] (and optional [`SpriteAnimation`]) from this slice definition.
    pub fn apply_to(&self, renderer: &mut SpriteRenderer, animation: Option<&mut SpriteAnimation>) {
        renderer.sprite_path = self.texture.clone();
        renderer.region_px = self.rect_px;
        renderer.frame_size = if self.rect_px[2] > 0 && self.rect_px[3] > 0 {
            [self.rect_px[2], self.rect_px[3]]
        } else {
            [0, 0]
        };
        renderer.sheet_columns = self.frames.max(1);
        renderer.sheet_row = 0;
        renderer.frame_index = 0;
        renderer.pivot = self.pivot;
        renderer.pixels_per_unit = if self.pixels_per_unit.is_finite() && self.pixels_per_unit > 0.0
        {
            self.pixels_per_unit
        } else {
            default_ppu()
        };

        if let Some(anim) = animation {
            anim.frames = self.frames.max(1);
            anim.frame_start = 0;
            anim.fps = self.fps.max(0.0);
            anim.looped = self.looped;
            anim.playing = self.frames > 1 && self.fps > 0.0;
        }
    }

    /// Build a fresh [`SpriteRenderer`] initialized from this slice definition.
    pub fn to_renderer(&self) -> SpriteRenderer {
        let mut renderer = SpriteRenderer::default();
        self.apply_to(&mut renderer, None);
        renderer
    }

    /// Build a matching [`SpriteAnimation`] for this slice definition.
    pub fn to_animation(&self) -> SpriteAnimation {
        SpriteAnimation {
            frames: self.frames.max(1),
            frame_start: 0,
            fps: self.fps.max(0.0),
            looped: self.looped,
            playing: self.frames > 1 && self.fps > 0.0,
            phase: 0.0,
        }
    }
}

/// Collection of named [`SpriteSliceDef`] entries loaded from JSON data files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpriteAtlasConfig {
    pub sprites: BTreeMap<String, SpriteSliceDef>,
}

impl SpriteAtlasConfig {
    /// Parse a JSON string into a [`SpriteAtlasConfig`].
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Look up a named slice definition.
    pub fn get(&self, key: &str) -> Option<&SpriteSliceDef> {
        self.sprites.get(key)
    }

    /// Apply a named slice definition to a [`SpriteRenderer`] and optional [`SpriteAnimation`].
    pub fn apply_to_renderer(
        &self,
        key: &str,
        renderer: &mut SpriteRenderer,
        animation: Option<&mut SpriteAnimation>,
    ) -> bool {
        let Some(def) = self.sprites.get(key) else {
            return false;
        };
        def.apply_to(renderer, animation);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_atlas_config_parses_and_applies_to_components() {
        let json = r#"{
            "sprites": {
                "hero_run": {
                    "texture": "sprites/atlas.png",
                    "rect_px": [0, 16, 16, 16],
                    "pivot": [0.5, 0.25],
                    "pixels_per_unit": 16.0,
                    "frames": 4,
                    "fps": 10.0,
                    "looped": true
                }
            }
        }"#;
        let cfg = SpriteAtlasConfig::from_json_str(json).expect("valid sprite atlas JSON");
        let mut renderer = SpriteRenderer::default();
        let mut anim = SpriteAnimation::default();
        assert!(cfg.apply_to_renderer("hero_run", &mut renderer, Some(&mut anim)));
        assert!(!cfg.apply_to_renderer("missing", &mut renderer, None));

        assert_eq!(renderer.sprite_path, "sprites/atlas.png");
        assert_eq!(renderer.region_px, [0, 16, 16, 16]);
        assert_eq!(renderer.frame_size_or([256, 256]), [16, 16]);
        assert_eq!(renderer.world_size([256, 256]), [1.0, 1.0]);
        assert_eq!(renderer.sheet_columns, 4);
        assert_eq!(renderer.pivot, [0.5, 0.25]);
        assert_eq!(anim.frames, 4);
        assert!(anim.playing);
        assert!((anim.fps - 10.0).abs() < 1e-6);
    }
}
