//! Persistent session log writer exported to `<project>/.cache/logs/`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::perf_report::{resolve_project_root, CalendarStamp, CACHE_DIR_NAME};

pub const LOG_SUBDIR_NAME: &str = "logs";

/// Returns `<project_root>/.cache/logs/`.
pub fn default_log_cache_dir() -> PathBuf {
    resolve_project_root()
        .join(CACHE_DIR_NAME)
        .join(LOG_SUBDIR_NAME)
}

/// Standard file naming rule: `emergence_log_<YYYYMMDD-HHMMSS>_pid<PID>.log`.
pub fn format_log_filename(stamp_compact: &str, pid: u32) -> String {
    format!("emergence_log_{stamp_compact}_pid{pid}.log")
}

fn parse_level_filter(raw: &str) -> Option<LevelFilter> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

fn resolve_levels(rust_log: Option<&str>) -> (LevelFilter, LevelFilter) {
    let stderr_level = rust_log
        .and_then(|s| parse_level_filter(s.split(',').next().unwrap_or(s)))
        .unwrap_or(LevelFilter::Error);
    let file_level = if stderr_level > LevelFilter::Info {
        stderr_level
    } else {
        LevelFilter::Info
    };
    (file_level, stderr_level)
}

struct FileTeeLogger {
    writer: Mutex<BufWriter<File>>,
    file_level: LevelFilter,
    stderr_level: LevelFilter,
}

impl FileTeeLogger {
    fn open(path: &Path, file_level: LevelFilter, stderr_level: LevelFilter) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
            file_level,
            stderr_level,
        })
    }
}

impl Log for FileTeeLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let lvl = metadata.level();
        lvl <= self.file_level || lvl <= self.stderr_level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let stamp = CalendarStamp::now().iso_display();
        let line = format!(
            "[{stamp} {:<5} {}] {}\n",
            record.level(),
            record.target(),
            record.args()
        );
        if record.level() <= self.file_level {
            if let Ok(mut guard) = self.writer.lock() {
                let _ = guard.write_all(line.as_bytes());
                let _ = guard.flush();
            }
        }
        if record.level() <= self.stderr_level {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.writer.lock() {
            let _ = guard.flush();
        }
        let _ = io::stderr().flush();
    }
}

/// Installs the persistent session logger writing to `<project>/.cache/logs/` and stderr.
///
/// Returns the created log file path on success, or `None` if opening the log file failed
/// or a process logger was already registered.
pub fn init_session_file_logger() -> Option<PathBuf> {
    let stamp = CalendarStamp::now().compact_for_filename();
    let pid = std::process::id();
    let path = default_log_cache_dir().join(format_log_filename(&stamp, pid));
    let rust_log = std::env::var("RUST_LOG").ok();
    let (file_level, stderr_level) = resolve_levels(rust_log.as_deref());
    let logger = match FileTeeLogger::open(&path, file_level, stderr_level) {
        Ok(logger) => logger,
        Err(err) => {
            eprintln!("[emergence][log] failed to open log file {}: {err}", path.display());
            return None;
        }
    };
    let max_level = file_level.max(stderr_level);
    if log::set_boxed_logger(Box::new(logger)).is_err() {
        return None;
    }
    log::set_max_level(max_level);
    log::info!(
        "emergence log initialized (pid={pid}, level={}, stderr={})",
        file_level,
        stderr_level
    );
    let _ = Level::Info;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_filename_and_level_resolution_match_cache_convention() {
        assert_eq!(
            format_log_filename("20261117-231800", 42),
            "emergence_log_20261117-231800_pid42.log"
        );
        assert_eq!(resolve_levels(None), (LevelFilter::Info, LevelFilter::Error));
        assert_eq!(resolve_levels(Some("debug")), (LevelFilter::Debug, LevelFilter::Debug));
        assert_eq!(resolve_levels(Some("warn")), (LevelFilter::Info, LevelFilter::Warn));
        assert!(default_log_cache_dir().ends_with(Path::new(".cache").join("logs")));
    }

    #[test]
    fn file_tee_logger_creates_and_flushes_log_entries() {
        let dir = std::env::temp_dir().join(format!("emergence_log_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("test.log");
        let logger = FileTeeLogger::open(&path, LevelFilter::Info, LevelFilter::Off).unwrap();
        let meta = log::MetadataBuilder::new()
            .level(Level::Info)
            .target("emergence::test")
            .build();
        let record = log::Record::builder()
            .metadata(meta)
            .args(format_args!("hello persistent log"))
            .build();
        logger.log(&record);
        logger.flush();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("INFO  emergence::test"));
        assert!(text.contains("hello persistent log"));
        let _ = fs::remove_dir_all(&dir);
    }
}
