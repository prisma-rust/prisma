//! Scene pack build stamp (#976).
//!
//! `assets/scenes.pak` contains, besides scenes and textures, a [`COOK_STAMP_NAME`] record
//! with component schema fingerprint and newest source document mtime at cook time. At runtime mounting, it is compared with
//! current engine / on-disk documents to give a clear hint when "pack is stale", rather than quietly
//! rendering a wrong image.
//!
//! Why needed: `.pak` header only has `version = 1`, two packs both written with v1
//! (one with old pipeline, one with new) are indistinguishable at runtime; old pack inside
//! `scenes/lake.rscn` still decodes, just content long out of sync with author document.
//!
//! Old packs without this record, parse returns [`StampCheck::Missing`] — still runnable,
//! but suggests re-cooking.

use serde::{Deserialize, Serialize};

/// Stamp path inside pack (relative to pack root).
pub const COOK_STAMP_NAME: &str = "__cook_stamp.json";

/// Current build stamp format version.
pub const COOK_STAMP_VERSION: u32 = 1;

/// One metadata written into `.pak` at cook time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookStamp {
    /// Format version (+1 when this file fields change).
    pub version: u32,
    /// [`crate::registry::ComponentRegistry::schema_fingerprint`].
    pub schema: u64,
    /// Newest mtime among author documents participating in cook (`scenes.json` / settings file / `*.scene.json` /
    /// `*.prefab.json`), Unix seconds.
    pub sources_mtime: u64,
}

impl Default for CookStamp {
    fn default() -> Self {
        Self {
            version: COOK_STAMP_VERSION,
            schema: 0,
            sources_mtime: 0,
        }
    }
}

impl CookStamp {
    pub fn new(schema: u64, sources_mtime: u64) -> Self {
        Self {
            version: COOK_STAMP_VERSION,
            schema,
            sources_mtime,
        }
    }

    /// Serialize to text written into pack (with newlines, for diff).
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    /// Parse stamp inside pack; returns `None` when fields missing or version unknown.
    pub fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let version = value.get("version")?.as_u64()? as u32;
        if version != COOK_STAMP_VERSION {
            return None;
        }
        Some(Self {
            version,
            schema: value.get("schema")?.as_u64()?,
            sources_mtime: value.get("sources_mtime")?.as_u64()?,
        })
    }
}

/// Conclusion of one check; caller (engine) responsible for translating into logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampCheck {
    /// Pack built by current engine + current documents.
    Fresh,
    /// Pack has no stamp: built by older pipeline, freshness cannot be determined.
    Missing,
    /// Component schema changed (engine upgrade / component field change), need re-cook.
    SchemaChanged { cooked: u64, current: u64 },
    /// Scene document newer than pack (scene changed but forgot to re-cook).
    SourcesNewer { cooked: u64, newest: u64 },
}

impl StampCheck {
    /// Whether user should be told to re-cook.
    pub fn needs_rebuild(&self) -> bool {
        !matches!(self, StampCheck::Fresh)
    }
}

/// Compare stamp with current state.
///
/// `current_schema` is current engine schema fingerprint; `newest_source` is newest mtime of project
/// documents on disk (release has no documents, pass 0 = skip freshness check).
///
/// mtime allows 2 seconds tolerance: Windows FAT / network drive second-level granularity could make
/// "just cooked" misjudged as stale.
pub fn check(stamp: Option<CookStamp>, current_schema: u64, newest_source: u64) -> StampCheck {
    let Some(stamp) = stamp else {
        return StampCheck::Missing;
    };
    if stamp.schema != current_schema {
        return StampCheck::SchemaChanged {
            cooked: stamp.schema,
            current: current_schema,
        };
    }
    if newest_source > stamp.sources_mtime.saturating_add(2) {
        return StampCheck::SourcesNewer {
            cooked: stamp.sources_mtime,
            newest: newest_source,
        };
    }
    StampCheck::Fresh
}

/// Scan newest mtime (Unix seconds) of "author documents that will be cooked into scene pack".
///
/// Only recognizes `*.scene.json` / `*.prefab.json` / `scenes.json` / the settings file:
/// loose textures backstopped by `.pak` own manifest, counting them would hint stale on every texture edit.
/// Release has none of these files, returns 0 (= skip freshness check).
pub fn newest_source_mtime(asset_root: &std::path::Path) -> u64 {
    fn visit(dir: &std::path::Path, newest: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, newest);
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let tracked = name.ends_with(".scene.json")
                || name.ends_with(".prefab.json")
                || name == "scenes.json"
                || name == "settings.ini";
            if !tracked {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
                continue;
            };
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            *newest = (*newest).max(secs);
        }
    }

    let mut newest = 0u64;
    visit(asset_root, &mut newest);
    newest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_round_trips_through_json() {
        let stamp = CookStamp::new(0xdead_beef, 1_700_000_000);
        let parsed = CookStamp::parse(&stamp.to_json()).expect("parse");
        assert_eq!(parsed, stamp);
    }

    #[test]
    fn unknown_or_broken_stamps_parse_to_none() {
        assert!(CookStamp::parse("").is_none());
        assert!(CookStamp::parse("{").is_none());
        assert!(CookStamp::parse(r#"{"version": 99, "schema": 1, "sources_mtime": 2}"#).is_none());
        assert!(CookStamp::parse(r#"{"version": 1}"#).is_none());
    }

    #[test]
    fn matching_stamp_is_fresh() {
        let stamp = CookStamp::new(7, 100);
        assert_eq!(check(Some(stamp), 7, 100), StampCheck::Fresh);
        assert!(!StampCheck::Fresh.needs_rebuild());
    }

    #[test]
    fn a_changed_schema_needs_a_rebuild() {
        let stamp = CookStamp::new(7, 100);
        assert_eq!(
            check(Some(stamp), 8, 100),
            StampCheck::SchemaChanged {
                cooked: 7,
                current: 8
            }
        );
    }

    #[test]
    fn a_newer_scene_document_needs_a_rebuild() {
        let stamp = CookStamp::new(7, 100);
        assert_eq!(
            check(Some(stamp), 7, 200),
            StampCheck::SourcesNewer {
                cooked: 100,
                newest: 200
            }
        );
        assert_eq!(check(Some(stamp), 7, 102), StampCheck::Fresh);
    }

    #[test]
    fn a_package_without_a_stamp_is_flagged() {
        assert_eq!(check(None, 7, 0), StampCheck::Missing);
        assert!(StampCheck::Missing.needs_rebuild());
    }

    #[test]
    fn shipped_builds_skip_the_source_check() {
        let stamp = CookStamp::new(7, 1_700_000_000);
        assert_eq!(check(Some(stamp), 7, 0), StampCheck::Fresh);
    }

    #[test]
    fn newest_source_mtime_only_looks_at_author_documents() {
        let dir = std::env::temp_dir().join(format!(
            "prism-cook-stamp-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(dir.join("scenes/a.scene.json"), b"{}").unwrap();
        std::fs::write(dir.join("settings.ini"), b"[app]\n").unwrap();
        std::fs::write(dir.join("scenes.pak"), b"pak").unwrap();

        let newest = newest_source_mtime(&dir);
        assert!(newest > 0, "Document should be scanned");

        std::fs::remove_dir_all(dir.join("scenes")).unwrap();
        std::fs::remove_file(dir.join("settings.ini")).unwrap();
        assert_eq!(newest_source_mtime(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
