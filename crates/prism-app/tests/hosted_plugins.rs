//! Hosted entry points must build registered plugins.
//!
//! `run` / `run_on` / `run_headless` all drain the pending plugin list before the
//! first frame. `start_hosted` did not, so a host driving the app itself (the
//! editor viewport path, the `hosted_png` / `pass_dump` examples) silently lost
//! every registered plugin: `with_plugin` accepted it, `build` never ran.

use prism_app::{App, AppPlugin};

#[derive(Debug, PartialEq)]
struct Marker(u32);

struct MarkerPlugin;

impl AppPlugin for MarkerPlugin {
    fn name(&self) -> &'static str {
        "MarkerPlugin"
    }

    fn build(&self, app: &mut App) {
        app.insert_resource(Marker(7));
    }
}

#[test]
fn start_hosted_builds_pending_plugins() {
    let mut app = App::hosted(prism_engine::config::AppConfig::default());
    app.add_plugin(MarkerPlugin);

    // Whatever the renderer does is irrelevant and async; the plugin drain happens
    // synchronously before it is asked for work.
    app.start_hosted(64, 64);

    assert_eq!(
        app.engine().world().get_resource::<Marker>(),
        Some(&Marker(7)),
        "start_hosted must drain pending plugins like every other entry point"
    );
    app.shutdown_hosted();
}
