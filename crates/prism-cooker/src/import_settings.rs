//! Per-asset import settings (#514): `<asset>.import.json` sidecar file.
//!
//! Format is small, covers only fields the cooker actually consumes:
//!
//! ```json
//! { "maxSize": 1024, "generateMips": false }
//! ```
//!
//! - Missing field = follow profile (not "off"); writing `null` equals missing;
//! - File name is `assets/sprites/boat.png.import.json`, same as Unity's `.meta` in
//!   "identity follows source file" concept, but doesn't introduce GUID;
//! - Editor (`prism-editor-egui`) uses same JSON keys for read/write, so this is also the single source of truth for the format
//!   (editor workspace doesn't link cooker, must sync these two keys via docs).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cooker::profile::CookSettings;

/// Texture import settings override (default = follow profile).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportSettings {
    /// Maximum edge length (pixels); `None` / `0` = unlimited.
    pub max_size: Option<u32>,
    /// Whether to generate mip chain; `None` = follow profile.
    pub generate_mips: Option<bool>,
}

impl ImportSettings {
    /// Sidecar path next to source file: `boat.png` → `boat.png.import.json`.
    pub fn sidecar_path(source: &Path) -> PathBuf {
        let mut path = source.as_os_str().to_owned();
        path.push(".import.json");
        PathBuf::from(path)
    }

    /// Read sidecar file; if missing / parse fails, return default value and warn (bad file doesn't break build).
    pub fn load(source: &Path) -> Self {
        let path = Self::sidecar_path(source);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!("import settings {} are invalid: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Write sidecar file (editor use). Parent directory must exist.
    pub fn save(&self, source: &Path) -> io::Result<()> {
        let path = Self::sidecar_path(source);
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Override onto cooker settings; `None` fields keep profile values.
    pub fn apply(&self, settings: &mut CookSettings) {
        if let Some(max_size) = self.max_size {
            settings.texture.max_size = max_size;
        }
        if let Some(generate_mips) = self.generate_mips {
            settings.texture.generate_mips = generate_mips;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_appends_to_the_extension() {
        assert_eq!(
            ImportSettings::sidecar_path(Path::new("assets/sprites/boat.png")),
            PathBuf::from("assets/sprites/boat.png.import.json")
        );
    }

    #[test]
    fn missing_or_broken_sidecars_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("prism-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("tex.png");
        std::fs::write(&source, b"not an image").unwrap();

        assert_eq!(ImportSettings::load(&source), ImportSettings::default());

        std::fs::write(ImportSettings::sidecar_path(&source), b"{ nope").unwrap();
        assert_eq!(ImportSettings::load(&source), ImportSettings::default());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_and_loading_round_trips_and_applies_only_present_fields() {
        let dir = std::env::temp_dir().join(format!("prism-import-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("tex.png");
        let settings = ImportSettings {
            max_size: Some(512),
            generate_mips: Some(false),
        };
        settings.save(&source).unwrap();
        assert_eq!(ImportSettings::load(&source), settings);

        let mut profile = CookSettings::default();
        settings.apply(&mut profile);
        assert_eq!(profile.texture.max_size, 512);
        assert!(!profile.texture.generate_mips);

        // Default fields don't override profile.
        ImportSettings::default().apply(&mut profile);
        assert_eq!(profile.texture.max_size, 512);

        std::fs::remove_dir_all(&dir).ok();
    }
}
