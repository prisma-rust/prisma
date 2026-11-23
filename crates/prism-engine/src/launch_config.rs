//! Launch config (hub → game param passing).
//!
//! Hub (Tauri launcher / Android hub) selects launch parameters before spawning game,
//! passed via unified JSON channel, engine side [`LaunchConfig::from_env`] auto-reads:
//!
//! - **Desktop**: launcher passes via `PRISMA_LAUNCH_CONFIG` env;
//! - **Android**: hub writes same JSON to app files dir
//!   `launch_config.json` (see Kotlin `NativePlugin.launch_game`),
//!   `android_main` reads then injects into env, unifying with desktop path.
//!
//! Parse failure or missing falls back to [`Default`], never blocks engine launch.

use serde::{Deserialize, Serialize};

/// Env var key used by application layer.
pub const ENV_KEY: &str = "PRISMA_LAUNCH_CONFIG";

/// Launch config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// Launch scene name (`scenes[].name` in `assets/scenes.json`).
    ///
    /// `None` = load first parseable scene in manifest (historical default).
    /// Editor / launcher can directly preview a specified scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// This process loads **no startup scene at all** (editor / tool host).
    ///
    /// Overrides [`Self::scene`]: when set, `scene` is ignored. Defaults to
    /// `false`, i.e. the historical "load the manifest's first resolvable scene"
    /// behaviour.
    ///
    /// This is not the same as `SceneManager`'s "scene missing" state: that means
    /// **a load was attempted and failed** (and paints the placeholder), while this
    /// means **no scene was ever wanted**.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_scene: bool,
    /// Log level override (e.g. `"debug"` / `"warn"`); `None` = platform default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

impl LaunchConfig {
    /// Launch config of current process (read [`ENV_KEY`]; missing/invalid falls back to default).
    pub fn from_env() -> Self {
        match std::env::var(ENV_KEY) {
            Ok(json) => Self::from_json(&json),
            Err(_) => Self::default(),
        }
    }

    /// Parse from caller-provided JSON; does not access env vars or file system.
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_else(|error| {
            log::warn!("invalid launch config ({error}); using defaults");
            Self::default()
        })
    }

    /// Serialize to JSON passed to hub (launcher side constructs params).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("LaunchConfig serializes")
    }

    /// Parse [`Self::log_level`] into a `log` filter.
    ///
    /// Only accepts single-level aliases (`off` / `error` / `warn` / `info` / `debug` / `trace`,
    /// case-insensitive); complex `RUST_LOG` directives continue to use env var.
    pub fn log_level_filter(&self) -> Option<log::LevelFilter> {
        self.log_level
            .as_deref()
            .and_then(|raw| raw.trim().parse().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::LaunchConfig;

    #[test]
    fn scene_is_optional_and_defaults_to_first_entry() {
        let launch = LaunchConfig::from_json("{}");
        assert_eq!(launch.scene, None);
        assert_eq!(launch.log_level, None);

        let launch = LaunchConfig::from_json(r#"{"scene":"default"}"#);
        assert_eq!(launch.scene.as_deref(), Some("default"));
    }

    #[test]
    fn invalid_json_falls_back_to_default() {
        let launch = LaunchConfig::from_json("not json");
        assert!(launch.scene.is_none());
        assert!(launch.log_level.is_none());
    }

    #[test]
    fn json_roundtrip_omits_absent_scene() {
        let json = LaunchConfig::default().to_json();
        assert_eq!(json, "{}");
    }

    #[test]
    fn no_scene_declares_a_host_without_a_startup_scene() {
        let launch = LaunchConfig::from_json(r#"{"no_scene":true}"#);
        assert!(launch.no_scene);
        assert_eq!(launch.scene, None);

        assert!(!LaunchConfig::default().no_scene);
        assert_eq!(LaunchConfig::default().to_json(), "{}");
    }

    #[test]
    fn log_level_filter_parses_case_insensitively() {
        let launch = LaunchConfig::from_json(r#"{"log_level":"DEBUG"}"#);
        assert_eq!(launch.log_level_filter(), Some(log::LevelFilter::Debug));

        let launch = LaunchConfig::from_json(r#"{"log_level":" warn "}"#);
        assert_eq!(launch.log_level_filter(), Some(log::LevelFilter::Warn));

        let launch = LaunchConfig::from_json(r#"{"log_level":"warn,foo=debug"}"#);
        assert_eq!(launch.log_level_filter(), None);
    }
}
