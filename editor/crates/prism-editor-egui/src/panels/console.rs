//! Console panel UI.

use super::*;

pub fn log(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let mut filter = workspace.log.filter().clone();
    filter.levels = state.log_levels.clone();
    filter.sources = state.log_sources.clone();
    filter.search = state.log_search.clone();
    if filter != *workspace.log.filter() {
        workspace.log.set_filter(filter);
    }

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.log_search)
                .hint_text(workspace.t("console.filterPlaceholder"))
                .desired_width(140.0),
        );
        ui.menu_button(workspace.t("console.levels"), |ui| {
            for level in [
                LogLevel::Error,
                LogLevel::Warn,
                LogLevel::Info,
                LogLevel::Debug,
                LogLevel::Trace,
                LogLevel::Panic,
            ] {
                let mut on = state.log_levels.contains(&level);
                let key = match level {
                    LogLevel::Error => "console.levelError",
                    LogLevel::Warn => "console.levelWarn",
                    LogLevel::Info => "console.levelInfo",
                    LogLevel::Debug => "console.levelDebug",
                    LogLevel::Trace => "console.levelTrace",
                    LogLevel::Panic => "console.levelPanic",
                };
                if ui.checkbox(&mut on, workspace.t(key)).changed() {
                    if on {
                        state.log_levels.push(level);
                    } else {
                        state.log_levels.retain(|item| *item != level);
                    }
                }
            }
        });
        ui.menu_button(workspace.t("console.sources"), |ui| {
            for source in [
                LogSource::Editor,
                LogSource::Engine,
                LogSource::Game,
                LogSource::System,
            ] {
                let mut on = state.log_sources.contains(&source);
                let key = match source {
                    LogSource::Editor => "console.editor",
                    LogSource::Engine => "desktop.console.engine",
                    LogSource::Game => "console.game",
                    LogSource::System => "desktop.console.system",
                };
                if ui.checkbox(&mut on, workspace.t(key)).changed() {
                    if on {
                        state.log_sources.push(source);
                    } else {
                        state.log_sources.retain(|item| *item != source);
                    }
                }
            }
        });
        ui.checkbox(&mut state.log_autoscroll, workspace.t("console.follow"));
        ui.checkbox(&mut state.log_collapse, workspace.t("console.collapse"));
        ui.checkbox(
            &mut state.log_clear_on_play,
            workspace.t("console.clearOnPlay"),
        );
        ui.checkbox(
            &mut state.log_error_pause,
            workspace.t("console.errorPause"),
        );
        if ui.button(workspace.t("console.clear")).clicked() {
            workspace.log.clear();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(workspace.tf(
                    "console.lines",
                    &[("count", &workspace.log.len().to_string())],
                ))
                .weak(),
            );
        });
    });
    ui.separator();

    let lines = workspace.log.visible_lines();
    let rows = log_rows(&lines, state.log_collapse);
    let row_height = 16.0;
    let mut jump: Option<(std::path::PathBuf, Option<usize>, Option<usize>)> = None;
    egui::ScrollArea::vertical()
        .id_salt("log-scroll")
        .auto_shrink([false, false])
        .stick_to_bottom(state.log_autoscroll)
        .show_rows(ui, row_height, rows.len(), |ui, range| {
            for (index, count) in &rows[range] {
                let line = &lines[*index];
                let color = match line.level {
                    LogLevel::Error | LogLevel::Panic => egui::Color32::from_rgb(232, 96, 96),
                    LogLevel::Warn => egui::Color32::from_rgb(224, 176, 80),
                    LogLevel::Info => egui::Color32::from_gray(200),
                    LogLevel::Debug | LogLevel::Trace => egui::Color32::from_gray(140),
                };
                let text = if *count > 1 {
                    format!("[{}] {}  (×{count})", line.source.label(), line.text)
                } else {
                    format!("[{}] {}", line.source.label(), line.text)
                };
                let response = ui.label(RichText::new(text).monospace().size(11.0).color(color));

                if log_location(&line.text).is_some() {
                    let response = response.interact(Sense::click());
                    if response.double_clicked() {
                        jump = log_location(&line.text).map(|(path, line, column)| {
                            (std::path::PathBuf::from(path), line, column)
                        });
                    }
                }
            }
        });
    if let Some((path, line, column)) = jump {
        state.status = match open_source_location(&path, line, column) {
            Ok(()) => format!("opened {}", path.display()),
            Err(error) => error,
        };
    }
}

pub(super) fn log_rows(
    lines: &[prism_editor_core::LogLine],
    collapse: bool,
) -> Vec<(usize, usize)> {
    if !collapse {
        return (0..lines.len()).map(|index| (index, 1)).collect();
    }
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut seen: std::collections::HashMap<(LogLevel, LogSource, &str), usize> =
        std::collections::HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        match seen.get(&(line.level, line.source, line.text.as_str())) {
            Some(&row) => rows[row].1 += 1,
            None => {
                seen.insert((line.level, line.source, line.text.as_str()), rows.len());
                rows.push((index, 1));
            }
        }
    }
    rows
}

pub(super) fn log_location(text: &str) -> Option<(String, Option<usize>, Option<usize>)> {
    for raw in text.split_whitespace() {
        let token = raw.trim_matches(|c: char| {
            matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\'' | '`')
        });

        let mut path = token;
        let mut numbers: Vec<usize> = Vec::new();
        while numbers.len() < 2 {
            let Some((head, tail)) = path.rsplit_once(':') else {
                break;
            };
            let Ok(number) = tail.parse::<usize>() else {
                break;
            };
            numbers.push(number);
            path = head;
        }
        if !path.contains('.') || !std::path::Path::new(path).is_file() {
            continue;
        }
        numbers.reverse();
        return Some((
            path.to_string(),
            numbers.first().copied(),
            numbers.get(1).copied(),
        ));
    }
    None
}

pub fn problems(
    ui: &mut Ui,
    workspace: &Workspace,
    module: &[Diagnostic],
    build: &[Diagnostic],
    state: &mut UiState,
) {
    let errors = module
        .iter()
        .chain(build)
        .filter(|diagnostic| diagnostic.level == LogLevel::Error)
        .count();
    let warnings = module.len() + build.len() - errors;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(workspace.tf(
                "desktop.problems.counts",
                &[
                    ("errors", &errors.to_string()),
                    ("warnings", &warnings.to_string()),
                ],
            ))
            .size(11.0),
        );
    });
    ui.separator();
    if module.is_empty() && build.is_empty() {
        ui.label(
            RichText::new(workspace.t("console.noProblems"))
                .weak()
                .size(11.0),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("problems-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            problem_section(ui, &workspace.t("desktop.problems.module"), module, state);
            problem_section(ui, &workspace.t("desktop.problems.build"), build, state);
        });
}

pub(super) fn problem_section(
    ui: &mut Ui,
    title: &str,
    diagnostics: &[Diagnostic],
    state: &mut UiState,
) {
    if diagnostics.is_empty() {
        return;
    }
    ui.label(RichText::new(title).size(10.5).weak());
    for diagnostic in diagnostics {
        let (icon, color) = match diagnostic.level {
            LogLevel::Error => (icons::X_CIRCLE, Color32::from_rgb(232, 96, 96)),
            _ => (icons::WARNING, Color32::from_rgb(224, 176, 80)),
        };
        let location = diagnostic.location();
        let mut response = ui
            .horizontal(|ui| {
                ui.label(RichText::new(icon).size(12.0).color(color));
                ui.label(RichText::new(&diagnostic.message).size(11.0));
                if let Some(location) = &location {
                    ui.label(RichText::new(location).weak().size(10.0));
                }
            })
            .response;
        if location.is_some() {
            response = response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                if let Some(file) = diagnostic.file.as_deref() {
                    state.status =
                        match open_source_location(file, diagnostic.line, diagnostic.column) {
                            Ok(()) => format!("opened {}", file.display()),
                            Err(error) => error,
                        };
                }
            }
        }
    }
    ui.add_space(4.0);
}

pub fn open_source_location(
    path: &std::path::Path,
    line: Option<usize>,
    column: Option<usize>,
) -> Result<(), String> {
    let file = path.display().to_string();
    let target = match line {
        Some(line) => match column {
            Some(column) => format!("{file}:{line}:{column}"),
            None => format!("{file}:{line}"),
        },
        None => file.clone(),
    };

    #[cfg(target_os = "windows")]
    let code_ready = on_path("code.cmd");
    #[cfg(not(target_os = "windows"))]
    let code_ready = on_path("code");
    if code_ready {
        let opened = if cfg!(target_os = "windows") {
            spawn_detached("cmd", &["/C", "code", "-g", &target])
        } else {
            spawn_detached("code", &["-g", &target])
        };
        if opened {
            return Ok(());
        }
    }
    let fallback = if cfg!(target_os = "windows") {
        spawn_detached("cmd", &["/C", "start", "", &file])
    } else if cfg!(target_os = "macos") {
        spawn_detached("open", &[&file])
    } else {
        spawn_detached("xdg-open", &[&file])
    };
    if fallback {
        Ok(())
    } else {
        Err(format!("cannot open {file}"))
    }
}

pub(super) fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

pub(super) fn spawn_detached(program: &str, args: &[&str]) -> bool {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); 
    }
    command.spawn().is_ok()
}

pub fn history(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let entries = workspace.history_entries();
    let cursor = workspace.history_cursor();
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut state.history_latest_first,
            workspace.t("console.latestFirst"),
        );
        ui.label(RichText::new(format!("{cursor}/{}", entries.len())).weak());
    });
    ui.separator();

    let mut jump: Option<usize> = None;
    let order: Vec<usize> = if state.history_latest_first {
        (0..entries.len()).rev().collect()
    } else {
        (0..entries.len()).collect()
    };
    egui::ScrollArea::vertical()
        .id_salt("history-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !state.history_latest_first {
                history_row(
                    ui,
                    workspace,
                    &workspace.t("console.initialState"),
                    cursor == 0,
                    cursor != 0,
                    0,
                    &mut jump,
                );
            }
            for index in &order {
                let (label, undone) = &entries[*index];

                let target = index + 1;
                let current = target == cursor;
                history_row(ui, workspace, label, current, *undone, target, &mut jump);
            }
            if state.history_latest_first {
                history_row(
                    ui,
                    workspace,
                    &workspace.t("console.initialState"),
                    cursor == 0,
                    cursor != 0,
                    0,
                    &mut jump,
                );
            }
        });
    if let Some(target) = jump {
        workspace.jump_history(target);
    }
}

pub(super) fn history_row(
    ui: &mut Ui,
    workspace: &Workspace,
    label: &str,
    current: bool,
    weak: bool,
    target: usize,
    jump: &mut Option<usize>,
) {
    let mut text = if weak {
        RichText::new(workspace.tf("console.undone", &[("label", label)])).weak()
    } else {
        RichText::new(label)
    };
    if current {
        text = text.strong();
    }
    if ui.selectable_label(current, text.size(11.0)).clicked() {
        *jump = Some(target);
    }
}
