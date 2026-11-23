//! Forward `log` records also to editor Console panel.
//!
//! `env_logger` still responsible for terminal output (`RUST_LOG` semantics unchanged); here wrap another layer
//! `log::Log`, put same record into a ring buffer, `EditorApp` takes per frame and writes into
//! `prism_editor_core::LogBuffer` (panel filters by source / level + windowed rendering).
//!
//! Without this, `prism_render` / `prism_engine` warnings in viewport only visible in terminal,
//! inconsistent with Web editor (in-process engine logs go to Console).

use std::collections::VecDeque;
use std::panic::PanicHookInfo;
use std::sync::{Mutex, OnceLock};

use prism_editor_core::{LogLevel, LogSource};

const CAPACITY: usize = 4_000;

struct Entry {
    source: LogSource,
    level: LogLevel,
    text: String,
}

/// Project module (preview module) log target prefix.
///
/// `prism-runtime-host` converts records emitted by module via `HostApi::log` into
/// `project: ...`; `panic` from crash hook in this file. Both belong to "game process",
/// Console panel filtering by source must separate from engine logs.
fn source_for(target: &str) -> LogSource {
    if target == "panic" || target.starts_with("project") {
        LogSource::Game
    } else {
        LogSource::Engine
    }
}

static SINK: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();

fn sink() -> &'static Mutex<VecDeque<Entry>> {
    SINK.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

struct Tee {
    inner: env_logger::Logger,
}

impl log::Log for Tee {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata) || metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.inner.enabled(record.metadata()) {
            self.inner.log(record);
        }
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Info => LogLevel::Info,
            log::Level::Debug => LogLevel::Debug,
            log::Level::Trace => return,
        };
        if let Ok(mut queue) = sink().lock() {
            if queue.len() >= CAPACITY {
                queue.pop_front();
            }
            queue.push_back(Entry {
                source: source_for(record.target()),
                level,
                text: format!("{}: {}", record.target(), record.args()),
            });
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Install tee logger and crash hook. Repeated calls are no-op.
pub fn install(default_filter: &str) {
    if SINK.get().is_some() {
        return;
    }
    sink();
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter))
            .format_timestamp(None)
            .build();
    let tee = Tee { inner };
    let max = log::LevelFilter::Debug;
    if log::set_boxed_logger(Box::new(tee)).is_ok() {
        log::set_max_level(max);
    }
    install_panic_hook();
}

/// Send panic call stack to Console panel.
///
/// Editor during Play loads project code as dynamic module into **same process**, so project's
/// `unwrap()` ends up here. Default hook only prints to stderr, panel cannot see; here additionally capture
/// one `Backtrace` (`force_capture`, not relying on `RUST_BACKTRACE`).
///
/// Whether module frames can be symbolized depends on PDB: `project_module::stage` copies `.pdb` and
/// `.dll` together to temp dir, dbghelp queries same-named symbol file by module dir.
fn install_panic_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
            push_panic(info);
            previous(info);
        }));
    });
}

fn push_panic(info: &PanicHookInfo<'_>) {
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let location = info
        .location()
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        })
        .unwrap_or_else(|| "<unknown location>".to_owned());
    let backtrace = std::backtrace::Backtrace::force_capture();
    if let Ok(mut queue) = sink().lock() {
        if queue.len() >= CAPACITY {
            queue.pop_front();
        }
        queue.push_back(Entry {
            source: LogSource::Game,
            level: LogLevel::Error,
            text: format!("panic at {location}: {payload}\n{backtrace}"),
        });
    }
}

/// Take new records from buffer (called once per frame).
pub fn drain() -> Vec<(LogSource, LogLevel, String)> {
    let Ok(mut queue) = sink().lock() else {
        return Vec::new();
    };
    queue
        .drain(..)
        .map(|entry| (entry.source, entry.level, entry.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_preserves_levels_and_source() {
        sink().lock().unwrap().clear();
        sink().lock().unwrap().push_back(Entry {
            source: LogSource::Engine,
            level: LogLevel::Warn,
            text: "engine warning".to_owned(),
        });
        let entries = drain();
        assert_eq!(
            entries,
            vec![(
                LogSource::Engine,
                LogLevel::Warn,
                "engine warning".to_owned()
            )]
        );
        assert!(drain().is_empty());
    }

    #[test]
    fn project_and_panic_targets_are_game_source() {
        assert_eq!(source_for("project"), LogSource::Game);
        assert_eq!(source_for("project:world"), LogSource::Game);
        assert_eq!(source_for("panic"), LogSource::Game);
        assert_eq!(source_for("prism_engine::scene"), LogSource::Engine);
        assert_eq!(source_for("prism_editor_egui::app"), LogSource::Engine);
    }
}
