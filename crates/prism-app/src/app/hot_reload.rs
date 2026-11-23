//! Development-time `.pak` hot-reload watcher setup.

use std::time::Duration;

/// Whether `PRISMA_HOT_RELOAD` value means "on" (#495).
///
/// Accepts `1` / `true` / `yes` / `on` (case-insensitive, leading/trailing whitespace ignored); other values (including `0`,
/// empty string, unset) all count as off. Release build defaults off, so runtime neither starts thread nor stats files.
pub(super) fn hot_reload_enabled(raw: Option<&str>) -> bool {
    raw.map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
    .unwrap_or(false)
}

/// When `PRISMA_HOT_RELOAD=1`, build a poller for all `.pak` under `assets/` (#495).
///
/// Release build defaults off: no thread, no stat, behavior completely same as before. Accepts `1` / `true` /
/// `yes` / `on` (case-insensitive), other values (including `0`, empty string) all count as off.
pub(super) fn spawn_pak_watcher() -> Option<prism_engine::scene::hot_reload::HotReloadWatcher> {
    let raw = std::env::var("PRISMA_HOT_RELOAD").ok();
    if !hot_reload_enabled(raw.as_deref()) {
        return None;
    }
    let paks = prism_engine::asset_paths::pak_files();
    if paks.is_empty() {
        log::warn!("hot reload: no `.pak` under assets/ to watch");
        return None;
    }
    match prism_engine::scene::hot_reload::HotReloadWatcher::watch_files_with_thread_setup(
        paks.clone(),
        Duration::from_millis(500),
        || {
            if !prism_platform::thread::put_behind_current() {
                log::debug!("hot reload poller keeps the default scheduling priority");
            }
        },
    ) {
        Ok(watcher) => {
            log::info!(
                "hot reload: watching {} package(s) for changes (development only)",
                paks.len()
            );
            Some(watcher)
        }
        Err(error) => {
            log::warn!("hot reload: cannot start the package watcher: {error}");
            None
        }
    }
}
