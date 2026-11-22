//! Project settings (`assets/settings.ini` `[app]` / `[window]` / `[build]` sections).
//!
//! Writing back **preserves other sections** in file (`[render]` etc.), only overwriting keys
//! these three sections are responsible for, consistent with Web editor `prism-editor-api::settings` behavior.
//!
//! Dialect see [`crate::ini`] (`docs/DESIGN.md` "Project Configuration Files"). Old project's
//! `assets/settings.toml` still readable (read-only fallback); write end only outputs `settings.ini`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ini::IniDocument;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub company: String,
    pub name: String,
    pub version: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            company: "Prisma".into(),
            name: "template".into(),
            version: "0.1.0".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    pub fullscreen: bool,
    pub vsync: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: "Prisma".into(),
            width: 1600,
            height: 900,
            resizable: true,
            fullscreen: false,
            vsync: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSettings {
    pub target: String,
    pub profile: String,
    pub output_dir: String,
    pub run_after: bool,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            target: "windows".into(),
            profile: "release".into(),
            output_dir: "build/windows".into(),
            run_after: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    pub app: AppSettings,
    pub window: WindowSettings,
    pub build: BuildSettings,
}

impl ProjectSettings {
    /// Write target: `assets/settings.ini`.
    pub fn path(root: &Path) -> PathBuf {
        root.join("assets/settings.ini")
    }

    /// Old project's `assets/settings.toml` (read-only fallback).
    pub fn legacy_path(root: &Path) -> PathBuf {
        root.join("assets/settings.toml")
    }

    /// Read; file not found returns defaults. Malformed lines skipped (INI parsing won't fail overall).
    pub fn read(root: &Path) -> io::Result<Self> {
        Ok(Self::from_document(&Self::read_document(root)?))
    }

    fn read_document(root: &Path) -> io::Result<IniDocument> {
        for (path, legacy) in [(Self::path(root), false), (Self::legacy_path(root), true)] {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    if legacy {
                        log::warn!(
                            "{} is deprecated: run `prism-build-pipeline build-scenes` to \
                             migrate it to settings.ini",
                            path.display()
                        );
                    }
                    return Ok(IniDocument::parse(&text));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(IniDocument::default())
    }

    pub fn from_document(document: &IniDocument) -> Self {
        let mut settings = Self::default();
        settings.app.company = string_or(document, "app", "company", &settings.app.company);
        settings.app.name = string_or(document, "app", "name", &settings.app.name);
        settings.app.version = string_or(document, "app", "version", &settings.app.version);

        settings.window.title = string_or(document, "window", "title", &settings.window.title);
        settings.window.width = int_or(document, "window", "width", settings.window.width);
        settings.window.height = int_or(document, "window", "height", settings.window.height);
        settings.window.resizable =
            bool_or(document, "window", "resizable", settings.window.resizable);
        settings.window.fullscreen =
            bool_or(document, "window", "fullscreen", settings.window.fullscreen);
        settings.window.vsync = bool_or(document, "window", "vsync", settings.window.vsync);

        settings.build.target = string_or(document, "build", "target", &settings.build.target);
        settings.build.profile = string_or(document, "build", "profile", &settings.build.profile);
        settings.build.output_dir =
            string_or(document, "build", "output_dir", &settings.build.output_dir);
        settings.build.run_after =
            bool_or(document, "build", "run_after", settings.build.run_after);
        settings
    }

    /// Merge write back: only overwrite keys we're responsible for in app/window/build, other sections preserved as-is.
    pub fn write(&self, root: &Path) -> io::Result<()> {
        let mut document = Self::read_document(root)?;

        document.set("app", "company", &self.app.company);
        document.set("app", "name", &self.app.name);
        document.set("app", "version", &self.app.version);

        document.set("window", "title", &self.window.title);
        document.set("window", "width", &self.window.width.to_string());
        document.set("window", "height", &self.window.height.to_string());
        document.set("window", "resizable", &self.window.resizable.to_string());
        document.set("window", "fullscreen", &self.window.fullscreen.to_string());
        document.set("window", "vsync", &self.window.vsync.to_string());

        document.set("build", "target", &self.build.target);
        document.set("build", "profile", &self.build.profile);
        document.set("build", "output_dir", &self.build.output_dir);
        document.set("build", "run_after", &self.build.run_after.to_string());

        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, document.render())
    }
}

/// Common values for `[build] target` (dropdown hints, doesn't restrict manual input).
pub const TARGET_CHOICES: &[&str] = &["windows", "android", "linux"];
/// Common values for `[build] profile` (consistent with `scripts/build-dist.*` profile names).
pub const PROFILE_CHOICES: &[&str] = &["debug", "release", "dist", "tiny"];

/// One editable field in the "Project Settings" panel.
///
/// This table is the **sole** field list: panel renders by it, search filters by it. "13 keys all editable"
/// promise is guarded by `every_written_key_is_editable` test -- add key to `write()` but forget table entry, test turns red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsField {
    pub section: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    /// Extra search keywords (Chinese / synonyms), not displayed.
    pub keywords: &'static str,
}

pub const SETTINGS_SECTIONS: &[&str] = &["app", "window", "build"];

pub const SETTINGS_FIELDS: &[SettingsField] = &[
    SettingsField {
        section: "app",
        key: "company",
        label: "Company",
        keywords: "company publisher",
    },
    SettingsField {
        section: "app",
        key: "name",
        label: "App name",
        keywords: "name app package",
    },
    SettingsField {
        section: "app",
        key: "version",
        label: "Version",
        keywords: "version",
    },
    SettingsField {
        section: "window",
        key: "title",
        label: "Window title",
        keywords: "title window",
    },
    SettingsField {
        section: "window",
        key: "width",
        label: "Window width",
        keywords: "width resolution size",
    },
    SettingsField {
        section: "window",
        key: "height",
        label: "Window height",
        keywords: "height resolution size",
    },
    SettingsField {
        section: "window",
        key: "resizable",
        label: "Resizable",
        keywords: "resizable stretch",
    },
    SettingsField {
        section: "window",
        key: "fullscreen",
        label: "Fullscreen",
        keywords: "fullscreen",
    },
    SettingsField {
        section: "window",
        key: "vsync",
        label: "V-Sync",
        keywords: "vertical sync sync",
    },
    SettingsField {
        section: "build",
        key: "target",
        label: "Build target",
        keywords: "platform target android windows",
    },
    SettingsField {
        section: "build",
        key: "profile",
        label: "Build profile",
        keywords: "config debug release dist tiny",
    },
    SettingsField {
        section: "build",
        key: "output_dir",
        label: "Output dir",
        keywords: "output directory path",
    },
    SettingsField {
        section: "build",
        key: "run_after",
        label: "Run after build",
        keywords: "run start play",
    },
];

/// Whether field matches query (label / key / section name / keyword, case-insensitive).
pub fn field_matches(field: &SettingsField, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    [field.label, field.key, field.section, field.keywords]
        .iter()
        .any(|text| text.to_lowercase().contains(&query))
}

/// Filter field table by search query; empty query returns all.
pub fn matching_fields(query: &str) -> Vec<&'static SettingsField> {
    SETTINGS_FIELDS
        .iter()
        .filter(|field| field_matches(field, query))
        .collect()
}

impl ProjectSettings {
    /// Restore defaults for one section (`app` / `window` / `build`).
    pub fn reset_section(&mut self, section: &str) {
        match section {
            "app" => self.app = AppSettings::default(),
            "window" => self.window = WindowSettings::default(),
            "build" => self.build = BuildSettings::default(),
            _ => {}
        }
    }
}

/// Panel's read cache: only parses on "first time / file content changed / manual reload",
/// edited-in values kept here, explicit Apply writes to disk (#428 items 1, 2, 5).
pub struct SettingsCache {
    root: PathBuf,
    /// Values being edited.
    pub settings: ProjectSettings,
    /// Disk values (synced after Apply).
    saved: ProjectSettings,
    /// Last read problem (parse / IO), shown on panel top.
    pub error: Option<String>,
    /// Original text last read; used to judge "did disk file change".
    ///
    /// Only comparing mtime misses same-second overwrites on coarse-grained filesystems (known flaky test
    /// cause); panel open decision is explicit, reading a 1-2 KB config file is fine.
    text: Option<String>,
    /// Disk changed during edit (panel hinted, but user input not overwritten).
    external_change: bool,
}

impl SettingsCache {
    pub fn load(root: impl Into<PathBuf>) -> Self {
        let mut cache = Self {
            root: root.into(),
            settings: ProjectSettings::default(),
            saved: ProjectSettings::default(),
            error: None,
            text: None,
            external_change: false,
        };
        cache.reload();
        cache
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn is_dirty(&self) -> bool {
        self.settings != self.saved
    }

    pub fn external_change(&self) -> bool {
        self.external_change
    }

    /// File changed on disk, re-read once. If unsaved edits exist, **don't overwrite** edited values,
    /// just set `external_change` to let panel hint.
    pub fn refresh(&mut self) -> bool {
        let current = read_settings_text(&self.root);
        if current == self.text {
            return false;
        }
        if self.is_dirty() {
            self.external_change = true;
            self.text = current;
            return false;
        }
        self.reload();
        true
    }

    fn reload(&mut self) {
        self.text = read_settings_text(&self.root);
        match ProjectSettings::read(&self.root) {
            Ok(settings) => {
                self.settings = settings.clone();
                self.saved = settings;
                self.error = None;
                self.external_change = false;
            }
            Err(error) => {
                self.error = Some(format!("read assets/settings.ini: {error}"));
                self.settings = ProjectSettings::default();
                self.saved = ProjectSettings::default();
            }
        }
    }

    /// Write to disk + sync "saved" snapshot.
    pub fn apply(&mut self) -> io::Result<()> {
        self.settings.write(&self.root)?;
        self.saved = self.settings.clone();
        self.error = None;
        self.external_change = false;
        self.text = read_settings_text(&self.root);
        Ok(())
    }

    /// Discard unsaved edits.
    pub fn revert(&mut self) {
        self.settings = self.saved.clone();
    }

    /// Discard unsaved edits and re-read everything from disk (file broken OK: reads defaults).
    pub fn reload_from_disk(&mut self) {
        self.settings = self.saved.clone();
        self.reload();
    }

    pub fn reset_section(&mut self, section: &str) {
        self.settings.reset_section(section);
    }

    pub fn reset_all(&mut self) {
        self.settings = ProjectSettings::default();
    }
}

/// Read raw text of `assets/settings.ini` (falling back to old `settings.toml`).
fn read_settings_text(root: &std::path::Path) -> Option<String> {
    for path in [
        ProjectSettings::path(root),
        ProjectSettings::legacy_path(root),
    ] {
        if let Ok(text) = fs::read_to_string(&path) {
            return Some(text);
        }
    }
    None
}

fn string_or(document: &IniDocument, section: &str, key: &str, fallback: &str) -> String {
    document
        .get_string(section, key)
        .unwrap_or_else(|| fallback.to_string())
}

fn int_or(document: &IniDocument, section: &str, key: &str, fallback: u32) -> u32 {
    document
        .get_u32(section, key)
        .map(|value| value.clamp(1, 16_384))
        .unwrap_or(fallback)
}

fn bool_or(document: &IniDocument, section: &str, key: &str, fallback: bool) -> bool {
    document.get_bool(section, key).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(1);

    struct TempRoot {
        root: PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-editor-core-settings-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("assets")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn every_written_key_is_editable() {
        let temp = TempRoot::new();
        ProjectSettings::default().write(&temp.root).unwrap();
        let text = fs::read_to_string(ProjectSettings::path(&temp.root)).unwrap();
        let document = IniDocument::parse(&text);
        for section in SETTINGS_SECTIONS {
            let keys: Vec<&str> = document
                .sections()
                .iter()
                .find(|entry| entry.name == *section)
                .map(|entry| entry.entries.iter().map(|(key, _)| key.as_str()).collect())
                .unwrap_or_default();
            assert!(!keys.is_empty(), "[{section}] is empty");
            for key in keys {
                assert!(
                    SETTINGS_FIELDS
                        .iter()
                        .any(|field| field.section == *section && field.key == key),
                    "[{section}] {key} is written but has no SettingsField entry"
                );
            }
        }
    }

    #[test]
    fn field_search_matches_labels_keys_and_chinese_keywords() {
        let hits = matching_fields("vsync");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "vsync");

        assert!(matching_fields("fullscreen")
            .iter()
            .any(|field| field.key == "fullscreen"));
        assert!(matching_fields("BUILD")
            .iter()
            .all(|field| field.section == "build"));
        assert_eq!(matching_fields("").len(), SETTINGS_FIELDS.len());
        assert!(matching_fields("nope-nothing").is_empty());
    }

    #[test]
    fn cache_only_reloads_when_the_file_changes() {
        let temp = TempRoot::new();
        ProjectSettings::default().write(&temp.root).unwrap();
        let mut cache = SettingsCache::load(&temp.root);
        assert!(!cache.refresh(), "nothing changed yet");

        let mut settings = ProjectSettings::default();
        settings.app.company = "Elsewhere".into();
        settings.write(&temp.root).unwrap();
        assert!(cache.refresh(), "file contents changed");
        assert_eq!(cache.settings.app.company, "Elsewhere");
        assert!(!cache.is_dirty());
    }

    #[test]
    fn cache_keeps_unsaved_edits_when_the_file_changes_underneath() {
        let temp = TempRoot::new();
        ProjectSettings::default().write(&temp.root).unwrap();
        let mut cache = SettingsCache::load(&temp.root);
        cache.settings.app.name = "edited".into();
        assert!(cache.is_dirty());

        // Same-length / same-second overwrites also need detecting (compare content, not mtime).
        let mut other = ProjectSettings::default();
        other.app.name = "external".into();
        other.write(&temp.root).unwrap();
        assert!(!cache.refresh(), "unsaved edits win");
        assert_eq!(cache.settings.app.name, "edited");
        assert!(cache.external_change());

        cache.revert();
        assert!(!cache.is_dirty());
        assert_eq!(cache.settings.app.name, AppSettings::default().name);
    }

    #[test]
    fn cache_reset_section_restores_only_that_section() {
        let temp = TempRoot::new();
        let mut cache = SettingsCache::load(&temp.root);
        cache.settings.app.name = "fishing".into();
        cache.settings.window.width = 800;
        cache.reset_section("window");
        assert_eq!(cache.settings.window.width, WindowSettings::default().width);
        assert_eq!(cache.settings.app.name, "fishing");

        cache.reset_all();
        assert_eq!(cache.settings, ProjectSettings::default());
        assert!(!cache.is_dirty());
    }

    #[test]
    fn apply_writes_the_edited_values() {
        let temp = TempRoot::new();
        let mut cache = SettingsCache::load(&temp.root);
        cache.settings.build.target = "android".into();
        cache.settings.window.fullscreen = true;
        cache.apply().unwrap();
        assert!(!cache.is_dirty());

        let reread = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(reread.build.target, "android");
        assert!(reread.window.fullscreen);
    }

    #[test]
    fn missing_file_reads_defaults() {
        let temp = TempRoot::new();
        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings, ProjectSettings::default());
    }

    #[test]
    fn write_creates_ini_and_roundtrips() {
        let temp = TempRoot::new();
        let mut settings = ProjectSettings::default();
        settings.app.company = "Acme".into();
        settings.window.width = 1280;
        settings.window.vsync = false;
        settings.build.run_after = true;

        settings.write(&temp.root).unwrap();
        let text = fs::read_to_string(ProjectSettings::path(&temp.root)).unwrap();
        assert!(text.contains("[app]"));
        assert!(text.contains("company = \"Acme\""));
        assert!(text.contains("width = 1280"));

        let read = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(read, settings);
    }

    #[test]
    fn write_preserves_other_sections_and_unknown_keys() {
        let temp = TempRoot::new();
        fs::write(
            ProjectSettings::path(&temp.root),
            r#"[app]
company = "Old"
custom = "keep-me"

[render]
render_mode = "path_trace"
debug_flags = 7

[window]
width = 800
title = "Old"
"#,
        )
        .unwrap();

        let mut settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings.app.company, "Old");
        assert_eq!(settings.window.width, 800);
        assert_eq!(
            settings.window.height, 900,
            "missing key uses default value"
        );

        settings.app.company = "New".into();
        settings.write(&temp.root).unwrap();

        let text = fs::read_to_string(ProjectSettings::path(&temp.root)).unwrap();
        assert!(text.contains("render_mode = \"path_trace\""));
        assert!(text.contains("debug_flags = 7"));
        assert!(text.contains("custom = \"keep-me\""));
        assert!(text.contains("company = \"New\""));
        assert!(text.contains("width = 800"));
    }

    #[test]
    fn legacy_toml_settings_are_readable() {
        let temp = TempRoot::new();
        fs::write(
            ProjectSettings::legacy_path(&temp.root),
            "[app]\ncompany = \"Legacy\"\n[window]\nwidth = 800\n",
        )
        .unwrap();

        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings.app.company, "Legacy");
        assert_eq!(settings.window.width, 800);

        // Write side only outputs settings.ini.
        settings.write(&temp.root).unwrap();
        assert!(ProjectSettings::path(&temp.root).is_file());
        assert!(ProjectSettings::legacy_path(&temp.root).is_file());
    }

    #[test]
    fn malformed_lines_fall_back_to_defaults() {
        let temp = TempRoot::new();
        fs::write(ProjectSettings::path(&temp.root), "[app\ncompany =").unwrap();
        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings, ProjectSettings::default());
    }

    #[test]
    fn out_of_range_numbers_are_clamped() {
        let temp = TempRoot::new();
        fs::write(
            ProjectSettings::path(&temp.root),
            "[window]\nwidth = 999999999\nheight = 0\n",
        )
        .unwrap();
        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings.window.width, 16_384);
        assert_eq!(settings.window.height, 1);
    }
}
