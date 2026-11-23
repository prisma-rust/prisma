//! Console log buffer (equivalent of `EditorLogger` / `preview-log`).
//!
//! The panel itself is windowed: only rows near the viewport are rendered, the rest use spacers as placeholders.
//! This provides all operations needed by the data side: ring buffer, level filtering, source filtering, search,
//! and "give me the index range near the viewport".

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Panic,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
            LogLevel::Panic => "panic",
        }
    }

    pub fn parse(text: &str) -> LogLevel {
        let lower = text.to_ascii_lowercase();
        if lower.contains("panic") {
            return LogLevel::Panic;
        }
        if lower.contains("error") {
            return LogLevel::Error;
        }
        if lower.contains("warn") {
            return LogLevel::Warn;
        }
        if lower.contains("debug") {
            return LogLevel::Debug;
        }
        if lower.contains("trace") {
            return LogLevel::Trace;
        }
        LogLevel::Info
    }

    /// Infer the level from an `env_logger`-style line (`[WARN] ...` / `WARN ...`).
    pub fn from_line(line: &str) -> LogLevel {
        let head: String = line.chars().take(24).collect();
        LogLevel::parse(&head)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogSource {
    /// The editor itself (UI / commands / docs).
    Editor,
    /// The engine (`prism_render` / `prism_engine` in the in-process viewport thread).
    Engine,
    /// The game process's stdout/stderr.
    Game,
    /// Process records produced by the editor (build, cook).
    System,
}

impl LogSource {
    pub fn label(self) -> &'static str {
        match self {
            LogSource::Editor => "editor",
            LogSource::Engine => "engine",
            LogSource::Game => "game",
            LogSource::System => "system",
        }
    }

    pub fn all() -> [LogSource; 4] {
        [
            LogSource::Editor,
            LogSource::Engine,
            LogSource::Game,
            LogSource::System,
        ]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub source: LogSource,
    pub level: LogLevel,
    pub text: String,
    /// Monotonically increasing sequence number (used as stable id for `uniform_list`).
    pub seq: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogFilter {
    /// Selected levels; empty = all.
    pub levels: Vec<LogLevel>,
    /// Selected sources; empty = all.
    pub sources: Vec<LogSource>,
    pub search: String,
}

impl LogFilter {
    pub fn accepts(&self, line: &LogLine) -> bool {
        if !self.levels.is_empty() && !self.levels.contains(&line.level) {
            return false;
        }
        if !self.sources.is_empty() && !self.sources.contains(&line.source) {
            return false;
        }
        if !self.search.is_empty()
            && !line
                .text
                .to_ascii_lowercase()
                .contains(&self.search.to_ascii_lowercase())
        {
            return false;
        }
        true
    }

    pub fn toggle_level(&mut self, level: LogLevel) {
        toggle(&mut self.levels, level);
    }

    pub fn toggle_source(&mut self, source: LogSource) {
        toggle(&mut self.sources, source);
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty() && self.sources.is_empty() && self.search.is_empty()
    }
}

fn toggle<T: PartialEq>(values: &mut Vec<T>, value: T) {
    match values.iter().position(|item| *item == value) {
        Some(index) => {
            values.remove(index);
        }
        None => values.push(value),
    }
}

#[derive(Debug, Clone)]
pub struct LogBuffer {
    lines: VecDeque<LogLine>,
    capacity: usize,
    filter: LogFilter,
    next_seq: u64,
    /// Increments on each append/clear, so the UI can tell whether it needs to recompute filter results.
    revision: u64,
    /// Cache of filtered results (rebuilt when `revision` changes).
    visible_cache: Vec<usize>,
    cache_revision: u64,
}

pub const DEFAULT_LOG_CAPACITY: usize = 5000;
pub const MAX_LOG_CAPACITY: usize = 20_000;

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_LOG_CAPACITY)
    }
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            capacity: capacity.clamp(1, MAX_LOG_CAPACITY),
            filter: LogFilter::default(),
            next_seq: 1,
            revision: 1,
            visible_cache: Vec::new(),
            cache_revision: 0,
        }
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.clamp(1, MAX_LOG_CAPACITY);
        while self.lines.len() > self.capacity {
            self.lines.pop_front();
        }
        self.revision += 1;
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn filter(&self) -> &LogFilter {
        &self.filter
    }

    pub fn filter_mut(&mut self) -> &mut LogFilter {
        self.revision += 1;
        &mut self.filter
    }

    pub fn set_filter(&mut self, filter: LogFilter) {
        self.filter = filter;
        self.revision += 1;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn push(&mut self, source: LogSource, level: LogLevel, text: impl Into<String>) {
        let line = LogLine {
            source,
            level,
            text: text.into(),
            seq: self.next_seq,
        };
        self.next_seq += 1;
        self.lines.push_back(line);
        while self.lines.len() > self.capacity {
            self.lines.pop_front();
        }
        self.revision += 1;
    }

    /// Append a line that is already text; level is inferred from the text prefix.
    pub fn push_line(&mut self, source: LogSource, text: impl Into<String>) {
        let text = text.into();
        let level = LogLevel::from_line(&text);
        self.push(source, level, text);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.revision += 1;
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&LogLine> {
        self.lines.get(index)
    }

    pub fn all(&self) -> &VecDeque<LogLine> {
        &self.lines
    }

    /// Number of filtered rows (panel scroll area height).
    pub fn visible_len(&mut self) -> usize {
        self.ensure_cache();
        self.visible_cache.len()
    }

    pub fn visible_line(&mut self, index: usize) -> Option<&LogLine> {
        self.ensure_cache();
        self.visible_cache
            .get(index)
            .and_then(|raw| self.lines.get(*raw))
    }

    /// Viewport window (frontend's `windowRange`): total rows, first row, last row.
    /// A snapshot of the current filtered results (`uniform_list`'s processor cannot borrow host state).
    pub fn visible_lines(&mut self) -> Vec<LogLine> {
        self.ensure_cache();
        self.visible_cache
            .iter()
            .filter_map(|index| self.lines.get(*index).cloned())
            .collect()
    }

    pub fn window(&mut self, scroll_top: f32, row_height: f32, viewport_height: f32) -> LogWindow {
        let total = self.visible_len();
        let first = if row_height <= 0.0 {
            0
        } else {
            (scroll_top / row_height).floor().max(0.0) as usize
        };
        let rows = if row_height <= 0.0 {
            total
        } else {
            (viewport_height / row_height).ceil() as usize + 1
        };
        let first = first.min(total);
        let last = (first + rows).min(total);
        LogWindow {
            total,
            first,
            last,
            top_spacer: first as f32 * row_height,
            bottom_spacer: (total.saturating_sub(last)) as f32 * row_height,
        }
    }

    pub fn error_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| matches!(line.level, LogLevel::Error | LogLevel::Panic))
            .count()
    }

    pub fn warn_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.level == LogLevel::Warn)
            .count()
    }

    fn ensure_cache(&mut self) {
        if self.cache_revision == self.revision {
            return;
        }
        self.visible_cache = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| self.filter.accepts(line))
            .map(|(index, _)| index)
            .collect();
        self.cache_revision = self.revision;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogWindow {
    pub total: usize,
    pub first: usize,
    pub last: usize,
    /// Top spacer height (pixels).
    pub top_spacer: f32,
    /// Bottom spacer height (pixels).
    pub bottom_spacer: f32,
}

pub const LOG_ROW_HEIGHT: f32 = 18.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_drops_oldest_lines() {
        let mut buffer = LogBuffer::new(3);
        for index in 0..5 {
            buffer.push(LogSource::Editor, LogLevel::Info, format!("line {index}"));
        }
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.get(0).unwrap().text, "line 2");
        assert_eq!(buffer.get(2).unwrap().text, "line 4");
    }

    #[test]
    fn capacity_is_clamped() {
        let buffer = LogBuffer::new(usize::MAX);
        assert_eq!(buffer.capacity(), MAX_LOG_CAPACITY);
        let buffer = LogBuffer::new(0);
        assert_eq!(buffer.capacity(), 1);
    }

    #[test]
    fn filters_by_level_source_and_search() {
        let mut buffer = LogBuffer::default();
        buffer.push(LogSource::Engine, LogLevel::Warn, "shadow map missing");
        buffer.push(LogSource::Game, LogLevel::Error, "panic in update");
        buffer.push(LogSource::Editor, LogLevel::Info, "saved scene");

        assert_eq!(buffer.visible_len(), 3);

        buffer.filter_mut().toggle_level(LogLevel::Info);
        assert_eq!(buffer.visible_len(), 1, "Info only");

        buffer.filter_mut().levels.clear();
        buffer.filter_mut().toggle_source(LogSource::Engine);
        assert_eq!(buffer.visible_len(), 1);
        assert_eq!(buffer.visible_line(0).unwrap().text, "shadow map missing");

        buffer.filter_mut().sources.clear();
        buffer.filter_mut().search = "PANIC".into();
        assert_eq!(buffer.visible_len(), 1, "search is case-insensitive");
    }

    #[test]
    fn cache_invalidates_on_new_lines() {
        let mut buffer = LogBuffer::default();
        buffer.push(LogSource::Editor, LogLevel::Info, "a");
        assert_eq!(buffer.visible_len(), 1);
        buffer.push(LogSource::Editor, LogLevel::Info, "b");
        assert_eq!(buffer.visible_len(), 2);

        buffer.filter_mut().search = "b".into();
        assert_eq!(buffer.visible_len(), 1);
        buffer.clear();
        assert_eq!(buffer.visible_len(), 0);
    }

    #[test]
    fn window_matches_fixed_row_height() {
        let mut buffer = LogBuffer::default();
        for index in 0..100 {
            buffer.push(LogSource::Editor, LogLevel::Info, format!("line {index}"));
        }
        let window = buffer.window(9.0 * LOG_ROW_HEIGHT, LOG_ROW_HEIGHT, 180.0);
        assert_eq!(window.total, 100);
        assert_eq!(window.first, 9);
        assert_eq!(window.last, 20, "180/18 = 10 rows + 1");
        assert_eq!(window.top_spacer, 9.0 * LOG_ROW_HEIGHT);
        assert_eq!(window.bottom_spacer, 80.0 * LOG_ROW_HEIGHT);

        let window = buffer.window(1_000.0 * LOG_ROW_HEIGHT, LOG_ROW_HEIGHT, 18.0);
        assert_eq!(window.first, 100);
        assert_eq!(window.last, 100);
        assert_eq!(window.bottom_spacer, 0.0);
    }

    #[test]
    fn counts_errors_and_warnings() {
        let mut buffer = LogBuffer::default();
        buffer.push(LogSource::Editor, LogLevel::Error, "e1");
        buffer.push(LogSource::Editor, LogLevel::Panic, "p1");
        buffer.push(LogSource::Editor, LogLevel::Warn, "w1");
        assert_eq!(buffer.error_count(), 2);
        assert_eq!(buffer.warn_count(), 1);
    }

    #[test]
    fn level_inferred_from_line_prefix() {
        assert_eq!(LogLevel::from_line("[ERROR] boom"), LogLevel::Error);
        assert_eq!(LogLevel::from_line("WARN  slow"), LogLevel::Warn);
        assert_eq!(LogLevel::from_line("[INFO] ok"), LogLevel::Info);
        assert_eq!(LogLevel::from_line("thread panicked"), LogLevel::Panic);
        assert_eq!(LogLevel::from_line("plain output"), LogLevel::Info);
    }

    #[test]
    fn clear_resets_visible_but_keeps_sequence_increasing() {
        let mut buffer = LogBuffer::default();
        buffer.push(LogSource::Editor, LogLevel::Info, "a");
        buffer.clear();
        buffer.push(LogSource::Editor, LogLevel::Info, "b");
        assert_eq!(buffer.len(), 1);
        assert!(buffer.get(0).unwrap().seq > 1);
    }
}
