//! Development-time `.pak` hot-reload watcher setup.

use std::time::Duration;

/// `PRISMA_HOT_RELOAD` 的取值是否表示「打开」（#495）。
///
/// 接受 `1` / `true` / `yes` / `on`（大小写、首尾空白都忽略）；其它值（含 `0`、
/// 空串、未设置）一律算关。发布版默认关，所以运行时既不起线程也不 stat 文件。
pub(super) fn hot_reload_enabled(raw: Option<&str>) -> bool {
    raw.map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
    .unwrap_or(false)
}

/// `PRISMA_HOT_RELOAD=1` 时给 `assets/` 下所有 `.pak` 建一个轮询器（#495）。
///
/// 发布版默认不建：没有线程、没有 stat，行为与之前完全一致。接受 `1` / `true` /
/// `yes` / `on`（大小写无关），其它值（含 `0`、空串）都当关。
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
