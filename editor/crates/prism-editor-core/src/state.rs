//! Editor local state persistence (corresponds to Web editor localStorage keys).
//!
//! The Web side stores three: `prisma-editor.dock.v4` / `prisma-editor.theme.v1` /
//! `prisma-editor.locale.v1` (plus preferences). The egui shell merges them into **one**
//! JSON file (default `<project>/target/editor-egui/state.json`), convenient for backup and troubleshooting.
//!
//! Semantics consistent with the frontend: theme stores only "preset + modified tokens"; layout goes through `DockLayout`
//! sanitize; unknown panels / illegal colors / nonexistent presets silently fall back.

use crate::dock::DockLayout;
use crate::i18n::Locale;
use crate::theme::{Rgba, ThemeToken, DEFAULT_THEME_PRESET, THEME_PRESETS};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const STATE_VERSION: u32 = 1;

/// Native preferences (same fields as the Web editor's preferences panel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    pub autosave: bool,
    pub confirm_delete: bool,
    pub compact: bool,
    /// Auto-rebuild scene packs after loose asset changes (#495). Default on: after changing
    /// textures / HDR / meshes you see it without exiting Play; turning it off avoids frequent repacking.
    pub hot_reload_assets: bool,
    /// Local control channel (MCP / automation, #1008). Default on; turning it off stops listening and deletes
    /// `control.port`; `--no-control` can also turn it off at startup.
    pub control_channel: bool,
    /// Shortcut overrides: `Command::id()` → key binding (`"Ctrl+Shift+S"`); empty = default table (#421).
    pub shortcut_overrides: BTreeMap<String, String>,
    pub max_log_lines: f32,
    pub locale_auto: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            autosave: true,
            confirm_delete: true,
            compact: false,
            hot_reload_assets: true,
            control_channel: true,
            shortcut_overrides: BTreeMap::new(),
            max_log_lines: 5000.0,
            locale_auto: true,
        }
    }
}

/// Scene view and render settings (#432): keeps toggles and tools across restarts.
///
/// Engine's `RenderSettings` is not owned by `core`, so here we only store an opaque JSON blob,
/// serialized/deserialized by the editor itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SceneSettings {
    pub show_grid: bool,
    pub show_skybox: bool,
    /// Projection wireframe for selected objects (#487); defaults to on for old archives lacking this key.
    #[serde(default = "default_selection_outline")]
    pub show_selection_outline: bool,
    /// Skeleton overlay for selected skinned entities (#560); defaults to on for old archives lacking this key.
    #[serde(default = "default_selection_outline")]
    pub show_skeleton: bool,
    /// `World` / `Local` (ASCII, avoid localized text creeping into archives).
    pub gizmo_space: String,
    /// `Translate` / `Rotate` / `Scale`.
    pub gizmo_tool: String,
    /// Mirror of engine `RenderSettings` as JSON; `None` = old archive, use defaults.
    pub render: Option<Value>,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            show_grid: true,
            show_skybox: true,
            show_selection_outline: default_selection_outline(),
            show_skeleton: default_selection_outline(),
            gizmo_space: "World".to_string(),
            gizmo_tool: "Translate".to_string(),
            render: None,
        }
    }
}

/// Old `state.json` had no `showSelectionOutline`; default to current behavior (on).
fn default_selection_outline() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub version: u32,
    /// `{version, dock, floating}` (same shape as Web's dock v4).
    pub dock: Value,
    pub theme_preset: String,
    /// token key (`bg` / `selection` …) → `#rrggbb`.
    #[serde(default)]
    pub theme_overrides: BTreeMap<String, String>,
    /// `zh-CN` / `en-US`.
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub preferences: Preferences,
    /// Scene view / render settings (`#[serde(default)]` ensures old `state.json` still reads).
    #[serde(default)]
    pub scene: SceneSettings,
    /// Maximized panel tab (`#[serde(default)]`: old archive = normal layout).
    #[serde(default)]
    pub maximized: Option<String>,
    /// Component presets (#418); empty table for old archives lacking them.
    #[serde(default)]
    pub component_presets: Vec<crate::workspace::ComponentPreset>,
}

fn default_locale() -> String {
    "zh-CN".to_string()
}

impl Default for EditorState {
    fn default() -> Self {
        let workspace = Workspace::sample();
        Self::capture(&workspace, &Preferences::default())
    }
}

impl EditorState {
    pub fn capture(workspace: &Workspace, preferences: &Preferences) -> Self {
        let mut theme_overrides = BTreeMap::new();
        for (token, color) in &workspace.theme.overrides {
            theme_overrides.insert(token.key().to_string(), color.to_hex());
        }
        Self {
            version: STATE_VERSION,
            dock: workspace.layout_json(),
            theme_preset: workspace.theme.preset.clone(),
            theme_overrides,
            locale: workspace.i18n.locale().code().to_string(),
            preferences: preferences.clone(),
            scene: SceneSettings {
                show_grid: workspace.show_grid,
                show_skybox: workspace.show_skybox,
                show_selection_outline: workspace.show_selection_outline,
                show_skeleton: workspace.show_skeleton,
                gizmo_space: match workspace.gizmo_space {
                    crate::viewport_math::GizmoSpace::Local => "Local".to_string(),
                    crate::viewport_math::GizmoSpace::World => "World".to_string(),
                },
                gizmo_tool: "Translate".to_string(),
                render: None,
            },
            maximized: workspace.maximized_tab.clone(),
            component_presets: workspace.component_presets.clone(),
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Tolerant read: missing fields use defaults, mismatched versions still recover known fields.
    pub fn from_json(value: &Value) -> Self {
        let mut state: Self =
            serde_json::from_value(value.clone()).unwrap_or_else(|_| Self::default());
        state.sanitize();
        state
    }

    pub fn from_json_str(text: &str) -> Self {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => Self::from_json(&value),
            Err(_) => Self::default(),
        }
    }

    /// Apply to a workspace (layout / theme / language / preferences).
    pub fn apply(&self, workspace: &mut Workspace, preferences: &mut Preferences) {
        let known = workspace.known_panels();
        let layout = DockLayout::from_json(&self.dock, &known, 1600.0, 900.0);
        workspace.dock = layout.dock;
        workspace.floating = layout.floating;

        workspace.theme.preset = if THEME_PRESETS
            .iter()
            .any(|preset| preset.id == self.theme_preset)
        {
            self.theme_preset.clone()
        } else {
            DEFAULT_THEME_PRESET.to_string()
        };
        workspace.theme.overrides.clear();
        for (key, hex) in &self.theme_overrides {
            let Some(token) = ThemeToken::ALL
                .iter()
                .find(|token| token.key() == key.as_str())
            else {
                continue;
            };
            if hex.len() == 7 && hex.starts_with('#') {
                workspace
                    .theme
                    .overrides
                    .insert(*token, Rgba::from_hex(hex));
            }
        }

        workspace.i18n.set_locale(Locale::parse(&self.locale));
        workspace.show_grid = self.scene.show_grid;
        workspace.show_skybox = self.scene.show_skybox;
        workspace.show_selection_outline = self.scene.show_selection_outline;
        workspace.show_skeleton = self.scene.show_skeleton;
        workspace.gizmo_space = match self.scene.gizmo_space.as_str() {
            "Local" | "local" => crate::viewport_math::GizmoSpace::Local,
            _ => crate::viewport_math::GizmoSpace::World,
        };
        // Panels may have been deleted (old archive / panel renamed); pointing to a missing tab
        // would blank the whole dock area, so filter against the current dock once.
        let maximized = self.maximized.clone().filter(|tab| {
            crate::dock::collect_tabs(&workspace.dock)
                .iter()
                .any(|t| t == tab)
        });
        workspace.maximized_tab = maximized;
        workspace.component_presets = self.component_presets.clone();

        *preferences = self.preferences.clone();
        preferences.max_log_lines = preferences.max_log_lines.clamp(500.0, 20_000.0);
    }

    fn sanitize(&mut self) {
        if self.theme_preset.is_empty() {
            self.theme_preset = DEFAULT_THEME_PRESET.to_string();
        }
        if !THEME_PRESETS
            .iter()
            .any(|preset| preset.id == self.theme_preset)
        {
            self.theme_preset = DEFAULT_THEME_PRESET.to_string();
        }
        self.theme_overrides.retain(|key, hex| {
            ThemeToken::ALL
                .iter()
                .any(|token| token.key() == key.as_str())
                && hex.len() == 7
                && hex.starts_with('#')
                && hex[1..].chars().all(|ch| ch.is_ascii_hexdigit())
        });
        if self.locale.is_empty() {
            self.locale = default_locale();
        }
        if !matches!(self.scene.gizmo_space.as_str(), "World" | "Local") {
            self.scene.gizmo_space = "World".to_string();
        }
        if !matches!(
            self.scene.gizmo_tool.as_str(),
            "Translate" | "Rotate" | "Scale"
        ) {
            self.scene.gizmo_tool = "Translate".to_string();
        }
        self.preferences.max_log_lines = self.preferences.max_log_lines.clamp(500.0, 20_000.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{DockEdge, DockZone};
    use serde_json::json;

    fn workspace() -> Workspace {
        Workspace::sample()
    }

    #[test]
    fn component_presets_survive_a_state_round_trip() {
        // #418: presets live in editor state, reopen editor after and they're still there.
        let mut source = Workspace::sample();
        source.save_component_preset(
            "prism_engine::scene::Camera",
            "wide",
            serde_json::json!({"fov_y_degrees": 90.0}),
        );
        let state = EditorState::capture(&source, &Preferences::default());
        let restored = EditorState::from_json_str(&state.to_json_string());

        let mut target = Workspace::sample();
        let mut prefs = Preferences::default();
        restored.apply(&mut target, &mut prefs);
        assert_eq!(
            target.component_preset_names("prism_engine::scene::Camera"),
            vec!["wide".to_string()]
        );
        assert_eq!(
            target.component_preset_value("prism_engine::scene::Camera", "wide"),
            Some(serde_json::json!({"fov_y_degrees": 90.0}))
        );
    }

    #[test]
    fn maximized_panel_survives_a_state_round_trip() {
        let mut source = workspace();
        source.toggle_maximized("sceneView");

        let state = EditorState::capture(&source, &Preferences::default());
        let mut target = Workspace::sample();
        let mut prefs = Preferences::default();
        state.apply(&mut target, &mut prefs);

        assert_eq!(target.maximized_tab.as_deref(), Some("sceneView"));

        // Archived panel no longer exists (renamed / layout changed): pointing at it must not
        // blank out the whole dock area.
        let mut stale = state.clone();
        stale.maximized = Some("no-such-panel".to_string());
        stale.apply(&mut target, &mut prefs);
        assert_eq!(target.maximized_tab, None);
    }

    #[test]
    fn roundtrip_layout_theme_locale_and_preferences() {
        let mut source = workspace();
        source.toggle_panel("log");
        let inspector = source.leaf_of("inspector").unwrap();
        source.dock_tab("log", &inspector, DockZone::Center);
        source.float_panel("preferences", 120.0, 140.0);
        source.set_theme_preset("paper");
        source
            .theme
            .overrides
            .insert(ThemeToken::Accent, Rgba::from_hex("#123456"));
        source.set_locale(Locale::EnUs);
        let mut prefs = Preferences {
            autosave: false,
            compact: true,
            max_log_lines: 1234.0,
            ..Default::default()
        };

        let state = EditorState::capture(&source, &prefs);
        let text = state.to_json_string();
        let restored = EditorState::from_json_str(&text);

        let mut target = workspace();
        let mut target_prefs = Preferences::default();
        restored.apply(&mut target, &mut target_prefs);

        assert_eq!(target.dock, source.dock);
        assert_eq!(target.floating, source.floating);
        assert_eq!(target.theme.preset, "paper");
        assert_eq!(
            target.theme.overrides.get(&ThemeToken::Accent),
            Some(&Rgba::from_hex("#123456"))
        );
        assert_eq!(target.i18n.locale(), Locale::EnUs);
        assert!(!target_prefs.autosave);
        assert!(target_prefs.compact);
        assert_eq!(target_prefs.max_log_lines, 1234.0);
        prefs.compact = true;
    }

    #[test]
    fn scene_settings_survive_roundtrip_and_old_state_uses_defaults() {
        let mut source = workspace();
        source.show_grid = false;
        source.show_skybox = false;
        source.show_selection_outline = false;
        source.show_skeleton = false;
        source.gizmo_space = crate::viewport_math::GizmoSpace::Local;
        let mut state = EditorState::capture(&source, &Preferences::default());
        state.scene.gizmo_tool = "Scale".to_string();
        state.scene.render = Some(json!({ "tonemapMode": 1, "pathTrace": true }));

        let restored = EditorState::from_json_str(&state.to_json_string());
        assert!(!restored.scene.show_grid);
        assert!(!restored.scene.show_skybox);
        assert!(!restored.scene.show_selection_outline);
        assert!(
            !restored.scene.show_skeleton,
            "skeleton toggle must be archived"
        );
        assert_eq!(restored.scene.gizmo_space, "Local");
        assert_eq!(restored.scene.gizmo_tool, "Scale");
        assert!(restored.scene.render.is_some());

        let mut target = workspace();
        let mut prefs = Preferences::default();
        restored.apply(&mut target, &mut prefs);
        assert!(!target.show_grid);
        assert!(!target.show_skybox);
        assert!(!target.show_selection_outline);
        assert!(!target.show_skeleton);
        assert_eq!(target.gizmo_space, crate::viewport_math::GizmoSpace::Local);

        // Old archive without `scene` section: defaults (outline defaults on).
        let old = json!({ "version": 1, "dock": {}, "themePreset": "studio" });
        let old_state = EditorState::from_json(&old);
        assert!(old_state.scene.show_grid);
        assert!(old_state.scene.show_selection_outline);
        assert!(
            old_state.scene.show_skeleton,
            "old archive defaults skeleton on"
        );
        assert_eq!(old_state.scene.gizmo_tool, "Translate");

        // Has `scene` section but missing `showSelectionOutline` (written by old version): default on.
        let partial = json!({
            "version": 1,
            "dock": {},
            "themePreset": "studio",
            "scene": { "showGrid": false }
        });
        let partial_state = EditorState::from_json(&partial);
        assert!(!partial_state.scene.show_grid);
        assert!(partial_state.scene.show_selection_outline);
    }

    #[test]
    fn control_channel_defaults_on_for_old_state_files() {
        let old = json!({ "version": 1, "dock": {}, "themePreset": "studio" });
        assert!(
            EditorState::from_json(&old).preferences.control_channel,
            "old archive has no controlChannel: default on, don't lock MCP out"
        );
        let off = json!({
            "version": 1,
            "dock": {},
            "themePreset": "studio",
            "preferences": { "controlChannel": false }
        });
        assert!(!EditorState::from_json(&off).preferences.control_channel);
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let value = json!({
            "version": 99,
            "dock": { "version": 4, "dock": { "type": "leaf", "tabs": ["ghost"], "active": "ghost" }, "floating": [] },
            "themePreset": "not-a-theme",
            "themeOverrides": { "accent": "salmon", "nope": "#ffffff", "bg": "#00ff00" },
            "locale": "fr-FR",
            "preferences": { "maxLogLines": 999999.0 }
        });
        let state = EditorState::from_json(&value);
        assert_eq!(state.theme_preset, DEFAULT_THEME_PRESET);
        assert_eq!(
            state.theme_overrides.len(),
            1,
            "only legal token + color kept"
        );
        assert!(state.theme_overrides.contains_key("bg"));
        assert_eq!(state.preferences.max_log_lines, 20_000.0);

        let mut workspace = workspace();
        let mut prefs = Preferences::default();
        state.apply(&mut workspace, &mut prefs);
        // Unknown panel is dropped, falls back to default layout (at least sceneView present).
        assert!(workspace.dock_tabs().iter().any(|tab| tab == "sceneView"));
        assert_eq!(workspace.i18n.locale(), Locale::ZhCn);
    }

    #[test]
    fn corrupt_json_returns_defaults() {
        let state = EditorState::from_json_str("{ not json");
        assert_eq!(state.version, STATE_VERSION);
        assert_eq!(state.theme_preset, DEFAULT_THEME_PRESET);
        assert!(state.preferences.autosave);
    }

    #[test]
    fn state_json_shape_is_stable() {
        let state = EditorState::capture(&workspace(), &Preferences::default());
        let value = state.to_json();
        assert_eq!(value["version"], STATE_VERSION);
        assert!(value["dock"]["dock"].is_object());
        assert_eq!(value["themePreset"], "studio");
        assert_eq!(value["locale"], "zh-CN");
        assert!(value["preferences"]["maxLogLines"].is_number());
    }

    #[test]
    fn edge_dock_and_float_survive_roundtrip() {
        let mut source = workspace();
        source.dock_tab_at_edge("log", DockEdge::Right);
        source.float_panel("build", 300.0, 200.0);
        let state = EditorState::capture(&source, &Preferences::default());
        let restored = EditorState::from_json_str(&state.to_json_string());
        let mut target = workspace();
        let mut prefs = Preferences::default();
        restored.apply(&mut target, &mut prefs);
        assert_eq!(target.dock, source.dock);
        assert_eq!(target.floating, source.floating);
    }
}
