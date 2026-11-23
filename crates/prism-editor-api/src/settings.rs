//! Project-level render settings (`[render]` section of the engine settings file).
//!
//! Read by `prism_app::load_config` at engine startup to override `RenderSettings`,
//! so fields here are the "editor → runtime" persistence contract. Read/write preserves other
//! sections in the file (`[app]` / `[window]` / `[build]`), but INI round-trip does not preserve comments.
//!
//! Here deliberately duplicates field definitions (rather than depending on `prism-engine`): editor backend
//! must stay Vulkan-free. Field names / values correspond one-to-one with `prism_engine::config::RenderConfig`.
//! One-to-one correspondence with the above.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ini::IniDocument;

/// Render settings for frontend use (camelCase JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSettingsData {
    /// `"raster"` or `"path_trace"`.
    pub render_mode: String,
    /// PBR isolate bit mask (0 = normal render).
    pub debug_flags: u32,
    /// 0 = Reinhard, 1 = ACES, 2 = passthrough (2D / pixel art, preserve source colors).
    pub tonemap_mode: u32,
    /// 0 = HDR tonemap, 1 = depth, 2 = view-space normal.
    pub debug_rt: u32,
    pub pt_max_bounces: u32,
    pub pt_ray_max_distance: f32,
    /// 0 = unlimited accumulation.
    pub pt_max_iterations: u32,
}

impl Default for RenderSettingsData {
    fn default() -> Self {
        Self {
            render_mode: "raster".to_string(),
            debug_flags: 0,
            tonemap_mode: 0,
            debug_rt: 0,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
        }
    }
}

impl RenderSettingsData {
    fn validate(&self) -> Result<(), String> {
        match self.render_mode.as_str() {
            "raster" | "path_trace" => {}
            other => {
                return Err(format!(
                    "render_mode can only be \"raster\" or \"path_trace\" (got {other:?})"
                ))
            }
        }
        if self.tonemap_mode > 2 {
            return Err(
                "tonemap_mode can only be 0 (Reinhard), 1 (ACES), or 2 (Passthrough)".into(),
            );
        }
        if self.debug_rt > 2 {
            return Err("debug_rt can only be 0 / 1 / 2".into());
        }
        if !(1..=64).contains(&self.pt_max_bounces) {
            return Err("pt_max_bounces must be in 1..=64".into());
        }
        if !(self.pt_ray_max_distance.is_finite() && self.pt_ray_max_distance > 0.0) {
            return Err("pt_ray_max_distance must be positive".into());
        }
        Ok(())
    }
}

/// Write target: the engine settings file.
pub fn settings_path(root: &Path) -> PathBuf {
    root.join("assets").join("settings.ini")
}

/// Read existing config document. Missing file returns empty document (= all default values).
fn read_document(root: &Path) -> Result<IniDocument, String> {
    let path = settings_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(IniDocument::parse(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(IniDocument::default()),
        Err(error) => Err(format!("Cannot read {}: {error}", path.display())),
    }
}

fn write_document(root: &Path, document: &IniDocument) -> Result<(), String> {
    let path = settings_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, document.render())
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))
}

/// Read `[render]` section (returns defaults when file / section missing).
pub fn read_render_settings(root: &Path) -> Result<RenderSettingsData, String> {
    let document = read_document(root)?;
    let mut data = RenderSettingsData::default();
    if let Some(value) = document.get_string("render", "render_mode") {
        data.render_mode = value;
    }
    data.debug_flags = document
        .get_u32("render", "debug_flags")
        .unwrap_or(data.debug_flags);
    data.tonemap_mode = document
        .get_u32("render", "tonemap_mode")
        .unwrap_or(data.tonemap_mode);
    data.debug_rt = document
        .get_u32("render", "debug_rt")
        .unwrap_or(data.debug_rt);
    data.pt_max_bounces = document
        .get_u32("render", "pt_max_bounces")
        .unwrap_or(data.pt_max_bounces);
    data.pt_ray_max_distance = document
        .get_f32("render", "pt_ray_max_distance")
        .unwrap_or(data.pt_ray_max_distance);
    data.pt_max_iterations = document
        .get_u32("render", "pt_max_iterations")
        .unwrap_or(data.pt_max_iterations);
    Ok(data)
}

/// Write `[render]` section (preserves other sections in file).
pub fn write_render_settings(root: &Path, data: &RenderSettingsData) -> Result<(), String> {
    data.validate()?;

    let mut document = read_document(root)?;
    document.set("render", "render_mode", &data.render_mode);
    document.set("render", "debug_flags", &data.debug_flags.to_string());
    document.set("render", "tonemap_mode", &data.tonemap_mode.to_string());
    document.set("render", "debug_rt", &data.debug_rt.to_string());
    document.set("render", "pt_max_bounces", &data.pt_max_bounces.to_string());
    document.set(
        "render",
        "pt_ray_max_distance",
        &format!("{:.1}", data.pt_ray_max_distance),
    );
    document.set(
        "render",
        "pt_max_iterations",
        &data.pt_max_iterations.to_string(),
    );
    write_document(root, &document)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsData {
    pub company: String,
    pub name: String,
    pub version: String,
}

impl Default for AppSettingsData {
    fn default() -> Self {
        Self {
            company: "Excurs1ons".into(),
            name: "Prisma".into(),
            version: "0.1.0".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSettingsData {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    pub fullscreen: bool,
    pub vsync: bool,
}

impl Default for WindowSettingsData {
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
#[serde(rename_all = "camelCase")]
pub struct BuildSettingsData {
    pub target: String,
    pub profile: String,
    pub output_dir: String,
    pub run_after: bool,
}

impl Default for BuildSettingsData {
    fn default() -> Self {
        Self {
            target: "windows".into(),
            profile: "release".into(),
            output_dir: "build/windows".into(),
            run_after: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettingsData {
    pub app: AppSettingsData,
    pub window: WindowSettingsData,
    pub build: BuildSettingsData,
}

impl ProjectSettingsData {
    fn validate(&self) -> Result<(), String> {
        if self.app.name.trim().is_empty() || self.app.company.trim().is_empty() {
            return Err("Application name and company name cannot be empty".into());
        }
        if self.window.title.trim().is_empty() {
            return Err("Window title cannot be empty".into());
        }
        if !(320..=16384).contains(&self.window.width)
            || !(240..=16384).contains(&self.window.height)
        {
            return Err("Window size must be in 320x240 .. 16384x16384".into());
        }
        if self.build.target != "windows" {
            return Err("Only windows build target currently supported".into());
        }
        if !matches!(self.build.profile.as_str(), "debug" | "release") {
            return Err("Build profile can only be debug or release".into());
        }
        if self.build.output_dir.trim().is_empty() {
            return Err("Output directory cannot be empty".into());
        }
        Ok(())
    }
}

/// Merge **changed leaves** between `before` → `after` onto `current`.
fn merge_changed_values(
    current: serde_json::Value,
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> serde_json::Value {
    if before == after {
        return current;
    }

    let (
        serde_json::Value::Object(mut current),
        serde_json::Value::Object(before),
        serde_json::Value::Object(after),
    ) = (current, before, after)
    else {
        return after.clone();
    };

    for (key, after_value) in after {
        if before.get(key) == Some(after_value) {
            continue;
        }
        let Some(before_value) = before.get(key) else {
            continue;
        };
        let Some(slot) = current.get_mut(key) else {
            continue;
        };
        let merged = merge_changed_values(std::mem::take(slot), before_value, after_value);
        if let Some(slot) = current.get_mut(key) {
            *slot = merged;
        }
    }

    serde_json::Value::Object(current)
}

fn merged_settings<T>(current: &T, before: &T, after: &T) -> Result<T, String>
where
    T: Serialize + DeserializeOwned,
{
    let to_value = |value: &T| {
        serde_json::to_value(value)
            .map_err(|error| format!("Failed to serialize settings: {error}"))
    };
    let merged = merge_changed_values(to_value(current)?, &to_value(before)?, &to_value(after)?);
    serde_json::from_value(merged).map_err(|error| format!("Failed to merge settings: {error}"))
}

/// Only write fields in `[app]`/`[window]`/`[build]` actually changed `before` → `after`.
///
/// Fields changed by other panels between reads keep disk value; `before == after` does not write file.
pub fn write_project_settings_diff(
    root: &Path,
    before: &ProjectSettingsData,
    after: &ProjectSettingsData,
) -> Result<(), String> {
    if before == after {
        return Ok(());
    }
    let current = read_project_settings(root)?;
    let merged = merged_settings(&current, before, after)?;
    write_project_settings(root, &merged)
}

/// Only write fields in `[render]` actually changed `before` → `after`.
pub fn write_render_settings_diff(
    root: &Path,
    before: &RenderSettingsData,
    after: &RenderSettingsData,
) -> Result<(), String> {
    if before == after {
        return Ok(());
    }
    let current = read_render_settings(root)?;
    let merged = merged_settings(&current, before, after)?;
    write_render_settings(root, &merged)
}

/// Read project settings (`[app]` / `[window]` / `[build]`); defaults when missing.
pub fn read_project_settings(root: &Path) -> Result<ProjectSettingsData, String> {
    let document = read_document(root)?;
    let mut data = ProjectSettingsData::default();

    if let Some(value) = document.get_string("app", "company") {
        data.app.company = value;
    }
    if let Some(value) = document.get_string("app", "name") {
        data.app.name = value;
    }
    if let Some(value) = document.get_string("app", "version") {
        data.app.version = value;
    }

    if let Some(value) = document.get_string("window", "title") {
        data.window.title = value;
    }
    data.window.width = document
        .get_u32("window", "width")
        .unwrap_or(data.window.width);
    data.window.height = document
        .get_u32("window", "height")
        .unwrap_or(data.window.height);
    data.window.resizable = document
        .get_bool("window", "resizable")
        .unwrap_or(data.window.resizable);
    data.window.fullscreen = document
        .get_bool("window", "fullscreen")
        .unwrap_or(data.window.fullscreen);
    data.window.vsync = document
        .get_bool("window", "vsync")
        .unwrap_or(data.window.vsync);

    if let Some(value) = document.get_string("build", "target") {
        data.build.target = value;
    }
    if let Some(value) = document.get_string("build", "profile") {
        data.build.profile = value;
    }
    if let Some(value) = document.get_string("build", "output_dir") {
        data.build.output_dir = value;
    }
    data.build.run_after = document
        .get_bool("build", "run_after")
        .unwrap_or(data.build.run_after);

    Ok(data)
}

/// Write project settings, preserving `[render]` and other sections.
pub fn write_project_settings(root: &Path, data: &ProjectSettingsData) -> Result<(), String> {
    data.validate()?;
    let mut document = read_document(root)?;

    document.set("app", "company", &data.app.company);
    document.set("app", "name", &data.app.name);
    document.set("app", "version", &data.app.version);

    document.set("window", "title", &data.window.title);
    document.set("window", "width", &data.window.width.to_string());
    document.set("window", "height", &data.window.height.to_string());
    document.set("window", "resizable", &data.window.resizable.to_string());
    document.set("window", "fullscreen", &data.window.fullscreen.to_string());
    document.set("window", "vsync", &data.window.vsync.to_string());

    document.set("build", "target", &data.build.target);
    document.set("build", "profile", &data.build.profile);
    document.set("build", "output_dir", &data.build.output_dir);
    document.set("build", "run_after", &data.build.run_after.to_string());

    write_document(root, &document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ini::IniDocument;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        dir
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = temp_dir("prism-editor-api-settings-missing");
        assert_eq!(
            read_render_settings(&dir).unwrap(),
            RenderSettingsData::default()
        );
    }

    #[test]
    fn roundtrip_preserves_other_sections() {
        let dir = temp_dir("prism-editor-api-settings-roundtrip");
        std::fs::write(
            settings_path(&dir),
            "[app]\nname = \"Demo\"\n\n[render]\nrender_mode = \"path_trace\"\n",
        )
        .unwrap();

        let mut data = read_render_settings(&dir).unwrap();
        assert_eq!(data.render_mode, "path_trace");
        data.pt_max_bounces = 5;
        write_render_settings(&dir, &data).unwrap();

        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        let document = IniDocument::parse(&text);
        assert_eq!(document.get("app", "name"), Some("Demo"));
        assert_eq!(read_render_settings(&dir).unwrap().pt_max_bounces, 5);
    }

    #[test]
    fn passthrough_tonemap_is_a_valid_project_setting() {
        let dir = temp_dir("prism-editor-api-settings-passthrough");
        let data = RenderSettingsData {
            tonemap_mode: 2,
            ..Default::default()
        };
        write_render_settings(&dir, &data).unwrap();
        assert_eq!(read_render_settings(&dir).unwrap().tonemap_mode, 2);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let dir = temp_dir("prism-editor-api-settings-invalid");
        let data = RenderSettingsData {
            tonemap_mode: 9,
            ..Default::default()
        };
        assert!(write_render_settings(&dir, &data).is_err());
    }

    #[test]
    fn project_settings_roundtrip_preserves_render() {
        let dir = temp_dir("prism-editor-api-project-settings-roundtrip");
        std::fs::write(
            settings_path(&dir),
            "[render]\nrender_mode = \"path_trace\"\n",
        )
        .unwrap();

        let mut data = read_project_settings(&dir).unwrap();
        assert_eq!(data.app.name, "Prisma");
        data.app.name = "DemoGame".into();
        data.window.width = 1280;
        data.build.profile = "debug".into();
        write_project_settings(&dir, &data).unwrap();

        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        let document = IniDocument::parse(&text);
        assert_eq!(document.get("app", "name"), Some("DemoGame"));
        assert_eq!(document.get_u32("window", "width"), Some(1280));
        assert_eq!(document.get("build", "profile"), Some("debug"));
        assert_eq!(document.get("render", "render_mode"), Some("path_trace"));
    }

    #[test]
    fn project_settings_reject_invalid_build_profile() {
        let dir = temp_dir("prism-editor-api-project-settings-invalid");
        let mut data = ProjectSettingsData::default();
        data.build.profile = "shipping".into();
        assert!(write_project_settings(&dir, &data).is_err());
    }

    #[test]
    fn project_settings_diff_keeps_changes_from_other_panels() {
        let dir = temp_dir("prism-editor-api-project-settings-diff-panels");
        write_project_settings(&dir, &ProjectSettingsData::default()).unwrap();

        let before = read_project_settings(&dir).unwrap();

        let mut other = read_project_settings(&dir).unwrap();
        other.app.name = "Renamed".into();
        write_project_settings(&dir, &other).unwrap();

        let mut after = before.clone();
        after.build.output_dir = "build/out".into();
        write_project_settings_diff(&dir, &before, &after).unwrap();

        let saved = read_project_settings(&dir).unwrap();
        assert_eq!(
            saved.app.name, "Renamed",
            "Section changed by another panel overwritten by old snapshot"
        );
        assert_eq!(saved.build.output_dir, "build/out");
    }

    #[test]
    fn project_settings_diff_keeps_fields_the_panel_did_not_touch() {
        let dir = temp_dir("prism-editor-api-project-settings-diff-fields");
        write_project_settings(&dir, &ProjectSettingsData::default()).unwrap();
        let before = read_project_settings(&dir).unwrap();

        let mut other = read_project_settings(&dir).unwrap();
        other.build.profile = "debug".into();
        write_project_settings(&dir, &other).unwrap();

        let mut after = before.clone();
        after.window.width = 1280;
        write_project_settings_diff(&dir, &before, &after).unwrap();

        let saved = read_project_settings(&dir).unwrap();
        assert_eq!(saved.window.width, 1280);
        assert_eq!(
            saved.build.profile, "debug",
            "Untouched field in same section overwritten by old value"
        );
    }

    #[test]
    fn project_settings_diff_does_not_rewrite_unchanged_snapshots() {
        let dir = temp_dir("prism-editor-api-project-settings-diff-noop");
        std::fs::write(settings_path(&dir), "# keep me\n[app]\nname = \"Demo\"\n").unwrap();

        let snapshot = read_project_settings(&dir).unwrap();
        write_project_settings_diff(&dir, &snapshot, &snapshot.clone()).unwrap();

        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(
            text.contains("# keep me"),
            "File should not be rewritten with no changes: {text}"
        );
    }

    #[test]
    fn render_settings_diff_only_writes_changed_fields() {
        let dir = temp_dir("prism-editor-api-render-settings-diff");
        write_render_settings(&dir, &RenderSettingsData::default()).unwrap();
        let before = read_render_settings(&dir).unwrap();

        let mut other = read_render_settings(&dir).unwrap();
        other.tonemap_mode = 1;
        write_render_settings(&dir, &other).unwrap();

        let mut after = before.clone();
        after.pt_max_bounces = 7;
        write_render_settings_diff(&dir, &before, &after).unwrap();

        let saved = read_render_settings(&dir).unwrap();
        assert_eq!(saved.pt_max_bounces, 7);
        assert_eq!(
            saved.tonemap_mode, 1,
            "Untouched field in same section overwritten by old value"
        );
    }
}
