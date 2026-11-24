//! Hot-reload support for the resource pipeline.
//!
//! Uses a simple polling approach (checks file metadata at intervals) so it
//! works on all platforms including Android/Termux without requiring inotify
//! or any OS-specific file-watch API.
//!
//! Runtime always compiles: poller only depends on `std`, no OS-specific file watch API needed.
//! (previously it hid behind `hot-reload` feature, only build-time crate opened that feature,
//! causing runtime to not get watcher -- see #495.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HotReloadError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Events reported by the hot-reload watcher.
#[derive(Debug, Clone)]
pub enum HotReloadEvent {
    /// A `.pak` file was modified — assets inside should be reloaded.
    PakModified(PathBuf),
    /// An unexpected watch error occurred.
    WatchError(String),
}

/// Polls one or more `.pak` files for changes at a configurable interval.
///
/// When a file's modification time changes, a [`HotReloadEvent::PakModified`]
/// event is emitted on the receiver channel.
///
/// ## Usage
///
/// ```ignore
/// let watcher = HotReloadWatcher::watch_file("game.pak", Duration::from_secs(1))?;
/// let mut rm = ResourceManager::new();
///
/// // In your game loop:
/// for event in watcher.receiver().try_iter() {
///     if let HotReloadEvent::PakModified(path) = event {
///         rm.on_pak_changed(&path)?;
///     }
/// }
/// ```
/// One file snapshot: modification time + length.
///
/// Only comparing mtime misses "rebuild within same second, length changed" packages (Android / Termux FUSE
/// FUSE timestamp granularity is 1 second), so length also participates in judgment (#495 item 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok()?,
        len: meta.len(),
    })
}

/// Whether new snapshot counts as "changed": mtime advanced more than 100ms, or length differs.
///
/// 100ms threshold avoids treating intermediate states during one write as multiple events; length change means content
/// really changed (rebuild within same second).
fn stamp_changed(old: Stamp, new: Stamp) -> bool {
    let newer = new
        .modified
        .duration_since(old.modified)
        .map(|delta| delta.as_millis() > 100)
        .unwrap_or(true);
    newer || new.len != old.len
}

pub struct HotReloadWatcher {
    /// Channel receiver for polling events.
    rx: mpsc::Receiver<HotReloadEvent>,
    /// Shared stop signal and condition variable to wake the poller immediately.
    stop: Arc<(Mutex<bool>, std::sync::Condvar)>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HotReloadWatcher {
    /// Create a new polling watcher for a single `.pak` file.
    pub fn watch_file(
        path: impl Into<PathBuf>,
        interval: Duration,
    ) -> Result<Self, HotReloadError> {
        let path: PathBuf = path.into();
        let paths = vec![path];
        Self::watch_files(paths, interval)
    }

    /// Create a new polling watcher for multiple `.pak` files.
    pub fn watch_files(paths: Vec<PathBuf>, interval: Duration) -> Result<Self, HotReloadError> {
        Self::watch_files_with_thread_setup(paths, interval, || {})
    }

    /// Create a new polling watcher for multiple `.pak` files, calling
    /// `on_thread_start` on the polling thread before the first poll.
    ///
    /// The hook lets the host apply a scheduling hint (for example lowering
    /// the polling thread behind the render thread) without this crate
    /// depending on a platform layer.
    pub fn watch_files_with_thread_setup(
        paths: Vec<PathBuf>,
        interval: Duration,
        on_thread_start: impl FnOnce() + Send + 'static,
    ) -> Result<Self, HotReloadError> {
        let (tx, rx) = mpsc::channel::<HotReloadEvent>();
        let tx_clone = tx.clone();
        let stop = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let stop_clone = stop.clone();

        let mut mtimes: HashMap<PathBuf, Stamp> = HashMap::new();
        for p in &paths {
            if let Some(stamp) = stamp_of(p) {
                mtimes.insert(p.clone(), stamp);
            }
        }

        let handle = thread::Builder::new()
            .name("hot-reload-poller".into())
            .spawn(move || {
                on_thread_start();
                let (lock, cvar) = &*stop_clone;
                loop {
                    let guard = match lock.lock() {
                        Ok(g) => g,
                        Err(e) => e.into_inner(),
                    };
                    if *guard {
                        return;
                    }
                    let (guard, _) = match cvar.wait_timeout_while(guard, interval, |s| !*s) {
                        Ok(res) => res,
                        Err(e) => e.into_inner(),
                    };
                    if *guard {
                        return;
                    }
                    drop(guard);

                    for p in &paths {
                        match stamp_of(p) {
                            Some(new) => {
                                let changed = match mtimes.get(p) {
                                    Some(&old) => stamp_changed(old, new),
                                    None => true,
                                };
                                if changed {
                                    mtimes.insert(p.clone(), new);
                                    let _ = tx_clone.send(HotReloadEvent::PakModified(p.clone()));
                                }
                            }
                            None => {
                                let _ = tx_clone.send(HotReloadEvent::WatchError(format!(
                                    "Cannot stat {}",
                                    p.display()
                                )));
                            }
                        }
                    }
                }
            })
            .map_err(|e| HotReloadError::Io(std::io::Error::other(e)))?;

        Ok(Self {
            rx,
            stop,
            thread: Some(handle),
        })
    }

    /// Get a reference to the event receiver.
    pub fn receiver(&self) -> &mpsc::Receiver<HotReloadEvent> {
        &self.rx
    }

    /// Stop the polling thread and wait for it to exit.
    pub fn stop(&mut self) {
        let (lock, cvar) = &*self.stop;
        if let Ok(mut flag) = lock.lock() {
            *flag = true;
        }
        cvar.notify_all();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for HotReloadWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
#[path = "hot_reload_tests.rs"]
mod tests;
