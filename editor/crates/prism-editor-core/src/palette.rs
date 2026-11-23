//! Global search (`Ctrl+K`) data layer.
//!
//! Editor has four separate search boxes (hierarchy / asset / Console / Add Component),
//! but "jump to an entity / open an asset / run a command" must first figure out which panel it's in.
//! Here workspace is flattened into a searchable entries table; sorting is a pure function (for testing);
//! popup drawing is in `prism-editor-egui/src/palette.rs`, doesn't depend on egui.
//!
//! No index: current scene entities and asset table are both in memory, linear scan is enough (#425).

use crate::keys::{Command, SHORTCUTS};
use crate::workspace::Workspace;

/// Entry category. Derived order is display order for empty query: jump to scene things first,
/// then documents and assets, last commands (commands generally filtered by name input).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteKind {
    Entity,
    Document,
    Asset,
    Command,
}

impl PaletteKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Command => "Command",
            Self::Document => "Document",
            Self::Entity => "Entity",
            Self::Asset => "Asset",
        }
    }
}

/// What to do after selecting a result. Data layer only describes intent, execution left to UI layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    RunCommand(Command),
    OpenDocument(usize),
    /// Entity **index** in current document (input param for `Workspace::select`).
    SelectEntity(usize),
    OpenAsset(String),
}

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub kind: PaletteKind,
    /// Main title (entity name / file name / human command name).
    pub title: String,
    /// Secondary info: hierarchy path, asset path, shortcut……
    pub detail: String,
    pub action: PaletteAction,
}

/// `ResetSceneCamera` → `Reset Scene Camera`.
///
/// `Command` name is Rust identifier (`ui.click`'s `command:<Name>` uses it for round-trip),
/// putting it directly in search box is unreadable.
pub fn humanize(identifier: &str) -> String {
    let mut out = String::with_capacity(identifier.len() + 4);
    for (index, ch) in identifier.chars().enumerate() {
        if index > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Command shortcut text (empty string if no binding).
pub fn shortcut_for(command: Command) -> &'static str {
    SHORTCUTS
        .iter()
        .find(|entry| entry.command == command)
        .map(|entry| entry.keys)
        .unwrap_or("")
}

/// Flatten workspace into entries table: command → open documents → current document's entities → assets.
pub fn entries(workspace: &Workspace) -> Vec<PaletteEntry> {
    let mut entries = Vec::new();

    for command in Command::ALL.iter().copied() {
        entries.push(PaletteEntry {
            kind: PaletteKind::Command,
            title: humanize(&format!("{command:?}")),
            detail: shortcut_for(command).to_string(),
            action: PaletteAction::RunCommand(command),
        });
    }

    let active = workspace.active_index();
    for (index, document) in workspace.docs.iter().enumerate() {
        let path = document
            .path
            .clone()
            .unwrap_or_else(|| "untitled".to_string());
        entries.push(PaletteEntry {
            kind: PaletteKind::Document,
            title: file_name(&path),
            detail: if index == active {
                format!("{path} (active)")
            } else {
                path
            },
            action: PaletteAction::OpenDocument(index),
        });
    }

    let doc = &workspace.document().doc;
    for (index, entity) in doc.entities.iter().enumerate() {
        let mut components: Vec<&str> = entity
            .components
            .keys()
            .map(|name| name.rsplit("::").next().unwrap_or(name))
            .collect();
        components.sort_unstable();
        let mut detail = entity_path(doc, index);
        if !components.is_empty() {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&components.join(", "));
        }
        entries.push(PaletteEntry {
            kind: PaletteKind::Entity,
            title: entity
                .name
                .clone()
                .unwrap_or_else(|| format!("entity {index}")),
            detail: detail.clone(),
            action: PaletteAction::SelectEntity(index),
        });
    }

    for asset in &workspace.assets {
        entries.push(PaletteEntry {
            kind: PaletteKind::Asset,
            title: file_name(&asset.path),
            detail: format!("{} · {}", asset.kind.label(), asset.path),
            action: PaletteAction::OpenAsset(asset.path.clone()),
        });
    }

    entries
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// `Parent/Child`, top-level entity returns empty string.
fn entity_path(doc: &crate::doc::SceneDoc, index: usize) -> String {
    let mut names = Vec::new();
    let mut current = doc.entities.get(index).and_then(|entity| entity.parent);
    let mut guard = doc.entities.len();
    while let Some(id) = current {
        if guard == 0 {
            break;
        }
        guard -= 1;
        let Some(parent_index) = doc.index_of_id(id) else {
            break;
        };
        let Some(parent) = doc.entities.get(parent_index) else {
            break;
        };
        names.push(parent.name.clone().unwrap_or_else(|| format!("#{id}")));
        current = parent.parent;
    }
    names.reverse();
    names.join("/")
}

/// Match rank, smaller is better; `None` = no match.
fn rank(query: &str, entry: &PaletteEntry) -> Option<(u8, usize)> {
    let title = entry.title.to_lowercase();
    let start = if title == query {
        0
    } else if title.starts_with(query) {
        1
    } else if title
        .split([' ', '/', '_', '-'])
        .any(|word| word.starts_with(query))
    {
        2
    } else if title.contains(query) {
        3
    } else if entry.detail.to_lowercase().contains(query) {
        5
    } else if subsequence(&title, query) {
        4
    } else {
        return None;
    };
    Some((start, entry.title.len()))
}

fn subsequence(text: &str, query: &str) -> bool {
    let mut chars = text.chars();
    query.chars().all(|needle| chars.any(|hay| hay == needle))
}

/// Sorted indices; empty query = show all grouped by category (command → document → entity → asset).
pub fn search(query: &str, entries: &[PaletteEntry]) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        let mut all: Vec<usize> = (0..entries.len()).collect();
        all.sort_by_key(|index| {
            (
                entries[*index].kind,
                entries[*index].title.to_lowercase(),
                *index,
            )
        });
        return all;
    }
    let mut scored: Vec<(u8, usize, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| rank(&query, entry).map(|(rank, len)| (rank, len, index)))
        .collect();
    scored.sort_unstable();
    scored.into_iter().map(|(_, _, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: PaletteKind, title: &str, detail: &str) -> PaletteEntry {
        PaletteEntry {
            kind,
            title: title.to_string(),
            detail: detail.to_string(),
            action: PaletteAction::OpenAsset(detail.to_string()),
        }
    }

    #[test]
    fn humanize_splits_camel_case() {
        assert_eq!(humanize("ResetSceneCamera"), "Reset Scene Camera");
        assert_eq!(humanize("Save"), "Save");
    }

    #[test]
    fn empty_query_lists_entities_then_assets_then_commands() {
        let entries = vec![
            entry(PaletteKind::Asset, "boat.png", "sprites/boat.png"),
            entry(PaletteKind::Entity, "boat", "water/boat"),
            entry(PaletteKind::Command, "Save", "Ctrl+S"),
        ];
        let order = search("", &entries);
        assert_eq!(order, vec![1, 0, 2]);
    }

    #[test]
    fn prefix_matches_outrank_substring_and_detail_matches() {
        let entries = vec![
            entry(
                PaletteKind::Asset,
                "water_tile.png",
                "sprites/water_tile.png",
            ),
            entry(PaletteKind::Entity, "water", "water"),
            entry(
                PaletteKind::Asset,
                "lake_water.tres",
                "materials/lake_water.tres",
            ),
            entry(PaletteKind::Asset, "boat.png", "sprites/water/boat.png"),
        ];
        let order = search("wat", &entries);
        assert_eq!(order, vec![1, 0, 2, 3]);
    }

    #[test]
    fn word_starts_beat_plain_contains() {
        let entries = vec![
            entry(PaletteKind::Entity, "boat_water", ""),
            entry(PaletteKind::Entity, "water", ""),
        ];
        assert_eq!(search("wat", &entries), vec![1, 0]);
    }

    #[test]
    fn subsequence_matches_typos_inside_a_word() {
        let entries = vec![
            entry(PaletteKind::Asset, "sprites/boat.png", ""),
            entry(PaletteKind::Asset, "sprites/lake.png", ""),
        ];
        assert_eq!(search("boat", &entries), vec![0]);
    }

    #[test]
    fn entries_cover_commands_documents_entities_and_assets() {
        let mut workspace = Workspace::sample();
        let all = entries(&workspace);
        assert!(all.iter().any(|entry| entry.kind == PaletteKind::Command
            && matches!(entry.action, PaletteAction::RunCommand(Command::Save))));
        assert!(all.iter().any(|entry| entry.kind == PaletteKind::Document));
        assert!(all.iter().any(|entry| entry.kind == PaletteKind::Entity));
        let entity = all
            .iter()
            .find(|entry| entry.kind == PaletteKind::Entity)
            .expect("entity entry");
        assert!(!entity.title.is_empty());

        workspace.refresh_assets();
        assert!(entries(&workspace)
            .iter()
            .all(|entry| !entry.title.is_empty()));
    }

    #[test]
    fn child_entities_show_their_parent_path() {
        let mut doc = crate::doc::SceneDoc::default();
        let parent = doc.add_named_child(None, "Boat");
        let child = doc.add_named_child(Some(parent), "Rod");
        assert_eq!(entity_path(&doc, child), "Boat");
        assert_eq!(entity_path(&doc, parent), "");
    }

    #[test]
    fn search_is_case_insensitive() {
        let entries = vec![entry(PaletteKind::Entity, "Boat", "")];
        assert_eq!(search("BOAT", &entries), vec![0]);
        assert_eq!(search("boat", &entries), vec![0]);
    }

    #[test]
    fn entities_report_their_component_types() {
        let mut doc = crate::doc::SceneDoc::default();
        let index = doc.add_named_child(None, "Lamp");
        doc.entities[index].components.insert(
            "prism_engine::scene::Light".to_string(),
            serde_json::json!({}),
        );
        assert_eq!(
            doc.entities[index]
                .components
                .keys()
                .map(|name| name.rsplit("::").next().unwrap())
                .collect::<Vec<_>>(),
            vec!["Light"]
        );
    }
}
