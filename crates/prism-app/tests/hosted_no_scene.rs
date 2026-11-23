//! Hosted (editor / tool) mode must not require a scene.
//!
//! A player entry point resolves `assets/scenes.json` at construction and marks the
//! engine "scene missing" when it cannot, which paints the full-window no-scene
//! placeholder. Hosts that own their documents must never take that path: the
//! editor opens files on demand and starts with an empty world.

use prism_app::App;

#[test]
fn hosted_startup_neither_loads_a_scene_nor_reports_one_missing() {
    let _ = env_logger::builder().is_test(true).try_init();

    let app = App::hosted(prism_engine::config::AppConfig::default());

    assert!(
        !app.engine().is_missing_scene(),
        "hosted mode must not attempt a startup scene load, so it cannot be 'missing'"
    );
    assert_eq!(app.engine().current_scene_name(), None);
}

#[test]
fn the_player_entry_point_does_report_a_missing_scene() {
    let _ = env_logger::builder().is_test(true).try_init();

    let app = App::with_config(prism_engine::config::AppConfig::default());
    assert!(
        app.engine().is_missing_scene(),
        "player entry point should report the missing startup scene"
    );
}

#[test]
fn a_launch_config_can_declare_no_startup_scene() {
    let launch = prism_engine::launch_config::LaunchConfig::from_json(r#"{"no_scene":true}"#);
    assert!(launch.no_scene);
}
