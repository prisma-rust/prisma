//! Parse result of `cargo --message-format=json`.
//!
//! Project module (Game View Play) and player build (`cargo build`) both take this path:
//! Editor only cares about error / warning + `file:line:col`, Problems panel click jumps.
//! note / help / build-script events are not "things to fix", drop directly.

use std::path::{Path, PathBuf};

use prism_editor_core::LogLevel;

/// Max diagnostics retained, avoid one bad file flooding panel.
pub const MAX_DIAGNOSTICS: usize = 100;

/// One compiler diagnostic. Keeps only error / warning, position is `file:line:col`.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub level: LogLevel,
    pub message: String,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Diagnostic {
    pub fn location(&self) -> Option<String> {
        let file = self.file.as_deref()?;
        let mut text = file.display().to_string();
        if let Some(line) = self.line {
            text.push_str(&format!(":{line}"));
            if let Some(column) = self.column {
                text.push_str(&format!(":{column}"));
            }
        }
        Some(text)
    }

    /// Console one-line summary (Console no rich text, position before message more eye-catching).
    pub fn console_line(&self) -> String {
        let level = match self.level {
            LogLevel::Error => "error",
            _ => "warning",
        };
        match self.location() {
            Some(location) => format!("{level}: {location}: {}", self.message),
            None => format!("{level}: {}", self.message),
        }
    }
}

/// Parse whole stdout (for one-shot build).
pub fn diagnostics_from_cargo(stdout: &str, root: &Path) -> Vec<Diagnostic> {
    stdout
        .lines()
        .filter_map(|line| parse_cargo_line(line, root))
        .take(MAX_DIAGNOSTICS)
        .collect()
}

/// Parse one cargo JSON line (streaming build per line). Returns `None` if not compile diagnostic.
pub fn parse_cargo_line(line: &str, root: &Path) -> Option<Diagnostic> {
    let value = serde_json::from_str::<serde_json::Value>(line.trim()).ok()?;
    if value.get("reason").and_then(|reason| reason.as_str()) != Some("compiler-message") {
        return None;
    }
    let message = value.get("message")?;
    let level = match message.get("level").and_then(|level| level.as_str()) {
        Some("error") => LogLevel::Error,
        Some("warning") => LogLevel::Warn,
        _ => return None,
    };
    let mut text = message
        .get("message")
        .and_then(|text| text.as_str())
        .unwrap_or_default()
        .to_string();
    if let Some(code) = message
        .get("code")
        .and_then(|code| code.get("code"))
        .and_then(|code| code.as_str())
    {
        text = format!("{code}: {text}");
    }
    let span = message
        .get("spans")
        .and_then(|spans| spans.as_array())
        .and_then(|spans| {
            spans
                .iter()
                .find(|span| {
                    span.get("is_primary").and_then(|primary| primary.as_bool()) == Some(true)
                })
                .or_else(|| spans.first())
        });
    let file = span
        .and_then(|span| span.get("file_name"))
        .and_then(|name| name.as_str())
        .map(|name| {
            let path = Path::new(name);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        });
    Some(Diagnostic {
        level,
        message: text,
        file,
        line: span
            .and_then(|span| span.get("line_start"))
            .and_then(|line| line.as_u64())
            .map(|line| line as usize),
        column: span
            .and_then(|span| span.get("column_start"))
            .and_then(|column| column.as_u64())
            .map(|column| column as usize),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cargo JSON diagnostics -> Problems panel wants file:line:col.
    #[test]
    fn cargo_diagnostics_carry_a_source_location() {
        let stdout = concat!(
            r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"src/game/world.rs","line_start":42,"column_start":9,"is_primary":true}]}}"#,
            "\n",
            r#"{"reason":"compiler-message","message":{"level":"note","message":"expected `f32`","spans":[]}}"#,
            "\n",
            r#"{"reason":"build-finished","success":false}"#,
            "\n",
            "not json at all",
        );
        let diagnostics = diagnostics_from_cargo(stdout, Path::new("/project"));
        assert_eq!(
            diagnostics.len(),
            1,
            "notes and cargo events are not problems"
        );
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.level, LogLevel::Error);
        assert_eq!(diagnostic.message, "E0308: mismatched types");
        assert_eq!(diagnostic.line, Some(42));
        assert_eq!(diagnostic.column, Some(9));
        assert!(
            diagnostic
                .file
                .as_deref()
                .is_some_and(|file| file.ends_with(Path::new("src/game/world.rs"))),
            "relative span paths are resolved against the project root"
        );
    }

    /// Keep warning even on successful compile: panel must not have content only on failure.
    #[test]
    fn warnings_survive_a_successful_build() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `state`","code":null,"spans":[{"file_name":"/abs/src/lib.rs","line_start":7,"column_start":13,"is_primary":true}]}}"#;
        let diagnostics = diagnostics_from_cargo(stdout, Path::new("/project"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].level, LogLevel::Warn);
        assert_eq!(
            diagnostics[0].file.as_deref(),
            Some(Path::new("/abs/src/lib.rs"))
        );
        assert_eq!(diagnostics[0].message, "unused variable: `state`");
    }

    /// Streaming parse must handle half-line noise (cargo occasionally mixes non-JSON lines).
    #[test]
    fn streaming_parser_ignores_noise() {
        assert!(parse_cargo_line("", Path::new("/project")).is_none());
        assert!(parse_cargo_line("Compiling game v0.1.0", Path::new("/project")).is_none());
        assert!(parse_cargo_line(
            r#"{"reason":"build-script-executed","package_id":"game 0.1.0"}"#,
            Path::new("/project")
        )
        .is_none());
    }
}
