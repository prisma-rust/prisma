//! Startup timing traces shared by the engine, the app layer and hosts.
//!
//! Every span is logged as `log::debug!` with `target: "startup"` and kept in a
//! process-wide table, so [`print_report`] can dump one ordered summary once the
//! first frame is on screen. Enable with `PRISMA_STARTUP_TRACE=1` or
//! `RUST_LOG=startup=debug`.
//!
//! The origin is the first call to [`init`] (or the first span, whichever comes
//! first), so hosts must call [`init`] from `main` to measure from process start.
//!
//! Per-frame spans (`app::tick_frame`, `editor::tick`) would otherwise drown the
//! one-shot startup spans, so each name is recorded at most [`REPEAT_LIMIT`]
//! times; the summary reports how many occurrences were dropped.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Origin of every startup measurement.
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// `PRISMA_STARTUP_TRACE` was set to an enabling value.
static FORCED: OnceLock<bool> = OnceLock::new();

/// Test-only override, because [`FORCED`] latches on the first probe.
#[cfg(test)]
static TEST_FORCE: AtomicBool = AtomicBool::new(false);

/// Set once the summary has been printed; recording stops afterwards.
static CLOSED: AtomicBool = AtomicBool::new(false);

/// Hard cap so a long-running host never grows the table without bound.
const MAX_RECORDS: usize = 4096;

/// How many occurrences of one name are recorded before it is dropped.
const REPEAT_LIMIT: u32 = 3;

/// Recording stops this long after the origin.
///
/// Startup work is what this trace exists for; leaving it on would turn the
/// per-frame spans into a log flood long after the window is up.
const WINDOW_MS: f64 = 10_000.0;

struct Record {
    at_ms: f64,
    duration_ms: f64,
    depth: u16,
    name: &'static str,
    detail: String,
}

static RECORDS: OnceLock<Mutex<Vec<Record>>> = OnceLock::new();

/// Occurrence count per name, used by [`REPEAT_LIMIT`].
static SEEN: OnceLock<Mutex<HashMap<&'static str, u32>>> = OnceLock::new();

thread_local! {
    static DEPTH: Cell<u16> = const { Cell::new(0) };
}

fn records() -> &'static Mutex<Vec<Record>> {
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

fn seen() -> &'static Mutex<HashMap<&'static str, u32>> {
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn forced() -> bool {
    *FORCED.get_or_init(|| {
        std::env::var("PRISMA_STARTUP_TRACE")
            .map(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

/// Whether tracing is active right now.
///
/// The environment check is latched; the `RUST_LOG` check is not, because the
/// logger may still be installed after the first probe.
pub fn enabled() -> bool {
    if CLOSED.load(Ordering::Relaxed) {
        return false;
    }
    #[cfg(test)]
    if TEST_FORCE.load(Ordering::Relaxed) {
        return now_ms() <= WINDOW_MS;
    }
    let active = forced() || log::log_enabled!(target: "startup", log::Level::Debug);
    if !active {
        return false;
    }
    if now_ms() > WINDOW_MS {
        CLOSED.store(true, Ordering::Relaxed);
        return false;
    }
    true
}

/// Fix the time origin. Call from `main` so spans measure from process start.
pub fn init() {
    EPOCH.get_or_init(Instant::now);
}

/// Milliseconds since the origin, as a float for sub-millisecond resolution.
pub fn now_ms() -> f64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// Count one occurrence of `name`; `true` while it is still under the budget.
fn admit(name: &'static str) -> bool {
    let Ok(mut table) = seen().lock() else {
        return false;
    };
    let count = table.entry(name).or_insert(0);
    *count += 1;
    *count <= REPEAT_LIMIT
}

fn push(name: &'static str, at_ms: f64, duration_ms: f64, depth: u16, detail: String) {
    let Ok(mut table) = records().lock() else {
        return;
    };
    if table.len() >= MAX_RECORDS {
        return;
    }
    table.push(Record {
        at_ms,
        duration_ms,
        depth,
        name,
        detail,
    });
}

fn emit(name: &'static str, at_ms: f64, duration_ms: f64, depth: u16, detail: &str) {
    let indent = "  ".repeat(depth as usize);
    log::debug!(
        target: "startup",
        "[startup] +{at_ms:>9.3}ms {indent}{name} {duration_ms:>8.3}ms{detail}"
    );
}

fn suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(" | {detail}")
    }
}

/// Record an instantaneous milestone.
pub fn mark(name: &'static str) {
    event(name, "");
}

/// Record an instantaneous milestone, building its detail only when tracing.
pub fn mark_with(name: &'static str, detail: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let at = now_ms();
    let depth = DEPTH.with(Cell::get);
    if !admit(name) {
        return;
    }
    let detail = suffix(&detail());
    emit(name, at, 0.0, depth, &detail);
    push(name, at, 0.0, depth, detail);
}

/// Record an instantaneous milestone with a pre-built detail string.
pub fn event(name: &'static str, detail: &str) {
    if !enabled() {
        return;
    }
    let at = now_ms();
    let depth = DEPTH.with(Cell::get);
    if !admit(name) {
        return;
    }
    let detail = suffix(detail);
    emit(name, at, 0.0, depth, &detail);
    push(name, at, 0.0, depth, detail);
}

/// Measures one named region; the duration is logged when the guard drops.
pub struct Span {
    name: &'static str,
    start: Instant,
    start_ms: f64,
    depth: u16,
    detail: String,
    /// Tracing is on: the guard owns one level of indentation.
    nesting: bool,
    /// This occurrence is inside the repeat budget and gets recorded.
    record: bool,
}

impl Span {
    /// Attach extra context (counts, sizes) printed with the duration.
    pub fn detail(&mut self, detail: impl Into<String>) {
        if self.record {
            self.detail = detail.into();
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.nesting {
            DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
        if !self.record {
            return;
        }
        let duration_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        let detail = suffix(&self.detail);
        emit(self.name, self.start_ms, duration_ms, self.depth, &detail);
        push(self.name, self.start_ms, duration_ms, self.depth, detail);
    }
}

/// Start measuring a region. Dropping the returned guard logs its duration.
pub fn span(name: &'static str) -> Span {
    let nesting = enabled();
    let depth = DEPTH.with(|depth| {
        let current = depth.get();
        if nesting {
            depth.set(current.saturating_add(1));
        }
        current
    });
    let record = nesting && admit(name);
    Span {
        name,
        start: Instant::now(),
        start_ms: now_ms(),
        depth,
        detail: String::new(),
        nesting,
        record,
    }
}

/// Print the ordered summary and stop recording.
///
/// Called once the first frame is presented; a second call is a no-op.
pub fn print_report() {
    if CLOSED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Ok(table) = records().lock() else {
        return;
    };
    if table.is_empty() {
        return;
    }
    let mut ordered: Vec<&Record> = table.iter().collect();
    ordered.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
    log::info!(
        target: "startup",
        "[startup] === trace: {} recorded events, {} distinct names (repeat limit {}) ===",
        ordered.len(),
        seen().lock().map(|table| table.len()).unwrap_or(0),
        REPEAT_LIMIT,
    );
    for record in ordered {
        let indent = "  ".repeat(record.depth as usize);
        log::info!(
            target: "startup",
            "[startup] +{:>9.3}ms {indent}{:<34} {:>9.3}ms{}",
            record.at_ms,
            record.name,
            record.duration_ms,
            record.detail
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// The trace tables are process-wide, so the tests that fill them must not
    /// run concurrently.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn reset() {
        // Without a logger the trace is off, so the tests force it on.
        TEST_FORCE.store(true, Ordering::Relaxed);
        CLOSED.store(false, Ordering::Relaxed);
        seen()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        records()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        init();
    }

    #[test]
    fn spans_record_their_duration() {
        let _guard = lock();
        reset();
        {
            let mut outer = span("test::outer");
            outer.detail("detail");
            let _inner = span("test::inner");
        }
        let table = records().lock().unwrap_or_else(|error| error.into_inner());
        let names: Vec<&str> = table.iter().map(|record| record.name).collect();
        assert!(names.contains(&"test::outer"));
        assert!(names.contains(&"test::inner"));
        let inner = table
            .iter()
            .find(|record| record.name == "test::inner")
            .expect("inner recorded");
        assert_eq!(inner.depth, 1);
    }

    #[test]
    fn repeated_names_stop_after_the_budget() {
        let _guard = lock();
        reset();
        for _ in 0..(REPEAT_LIMIT + 4) {
            event("test::repeat", "");
        }
        let table = records().lock().unwrap_or_else(|error| error.into_inner());
        let recorded = table
            .iter()
            .filter(|record| record.name == "test::repeat")
            .count();
        assert_eq!(recorded as u32, REPEAT_LIMIT);
    }

    #[test]
    fn the_epoch_is_stable() {
        let _guard = lock();
        init();
        let first = now_ms();
        let second = now_ms();
        assert!(second >= first);
    }
}
