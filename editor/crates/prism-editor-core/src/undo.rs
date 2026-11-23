//! Undo / redo stack.
//!
//! Two command types:
//! - `SetField`: A single field write in the inspector. The same entity + component + path changes within
//!   `coalesce_ms` coalesce into one step (dragging a slider only produces one undo).
//! - `Snapshot`: Structural edits (add/remove entities / reorder parents / prefab instantiation / paste components).
//!   The document is small; whole-document snapshots are less error-prone than per-operation inverse computation.

use crate::doc::{SceneDoc, TransformJson};
use crate::schema::set_at_path;
use serde_json::Value;

const DEFAULT_LIMIT: usize = 200;
const DEFAULT_COALESCE_MS: u64 = 800;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SetField {
        entity_id: u32,
        component: String,
        path: String,
        before: Value,
        after: Value,
    },
    /// Transform is the structural field of author JSON (not in `components`), a separate command type.
    Transform {
        entity_id: u32,
        before: TransformJson,
        after: TransformJson,
    },
    /// Multi-select gizmo drag: only one undo step per frame; merge by key during drag.
    TransformBatch {
        changes: Vec<(u32, TransformJson, TransformJson)>,
    },
    Snapshot {
        before: Box<SceneDoc>,
        after: Box<SceneDoc>,
    },
}

impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Command::SetField { .. } => "Edit field",
            Command::Transform { .. } => "Edit Transform",
            Command::TransformBatch { .. } => "Edit Transform",
            Command::Snapshot { .. } => "Edit scene",
        }
    }
}

fn apply_field(
    doc: &mut SceneDoc,
    entity_id: u32,
    component: &str,
    path: &str,
    value: &Value,
) -> bool {
    let Some(index) = doc.index_of_id(entity_id) else {
        return false;
    };
    let Some(root) = doc.entities[index].components.get_mut(component) else {
        return false;
    };
    set_at_path(root, path, value.clone())
}

/// Selected entities before and after one structural edit (by `EntityJson.id`, not array index).
///
/// Undo / redo restores the selection too, see #513: delete → undo re-selects the original entity,
/// copy → undo returns to the pre-copy selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectionSnapshot {
    pub before: Vec<u32>,
    pub after: Vec<u32>,
}

/// [`UndoStack::undo`] / [`UndoStack::redo`] result: label + selection to restore (if any).
#[derive(Debug, Clone, PartialEq)]
pub struct UndoOutcome {
    pub label: String,
    /// Selected entity ids saved for this step (`None` for field/transform edits).
    pub selection: Option<Vec<u32>>,
    /// True when only entity transforms changed, allowing in-place ECS sync.
    pub transform_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub command: Command,
    pub label: String,
    coalesce: Option<String>,
    at_ms: u64,
    /// Selection snapshot for structural edits; `None` for field / Transform commands (entity set unchanged).
    selection: Option<SelectionSnapshot>,
    /// Document state number after executing / undoing this step.
    ///
    /// Numbers are monotonically increasing and globally unique, so "undo back to the saved point"
    /// and "undo then do a different edit at the same depth" can be distinguished — comparing only
    /// the undo-stack depth would misjudge the latter as unmodified (#992).
    before_state: u64,
    after_state: u64,
}

impl Entry {
    pub fn is_field_edit(&self) -> bool {
        matches!(
            self.command,
            Command::SetField { .. } | Command::Transform { .. } | Command::TransformBatch { .. }
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct UndoStack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
    coalesce_ms: u64,
    /// Interactive in progress (dragging gizmo / dragging input field): ignore the coalesce time window,
    /// the entire interaction always compresses into one step.
    sticky: bool,
    /// Next available state number.
    next_state: u64,
    /// Document state number.
    current_state: u64,
    /// State number at last `mark_saved`; `None` = current state not reachable (save point discarded).
    saved_state: Option<u64>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: DEFAULT_LIMIT,
            coalesce_ms: DEFAULT_COALESCE_MS,
            sticky: false,
            next_state: 1,
            current_state: 0,
            saved_state: Some(0),
        }
    }

    fn allocate_state(&mut self) -> u64 {
        let state = self.next_state;
        self.next_state += 1;
        state
    }

    /// Whether the current document state differs from the last `mark_saved` baseline.
    ///
    /// Looking only at the undo cursor / depth is not enough: undoing back to the save point makes
    /// `undo` and `redo` lengths match the save time; but if a different edit is then made at the
    /// same depth, depth matches yet content differs. The state number covers both cases.
    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.current_state)
    }

    /// Mark the current state as the "saved" baseline.
    pub fn mark_saved(&mut self) {
        self.saved_state = Some(self.current_state);
    }

    /// Begin an interaction: subsequent changes are not subject to the `coalesce_ms` time window,
    /// the whole interaction compresses into one step.
    pub fn begin_interaction(&mut self) {
        self.sticky = true;
    }

    /// End an interaction: clear the merge mark on the last step, so the next modification must open a new step.
    pub fn commit(&mut self) {
        self.sticky = false;
        if let Some(last) = self.undo.last_mut() {
            last.coalesce = None;
        }
    }

    /// Whether an interaction is in progress (for testing / debugging).
    pub fn interaction_active(&self) -> bool {
        self.sticky
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    pub fn set_coalesce_ms(&mut self, ms: u64) {
        self.coalesce_ms = ms;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|entry| entry.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }

    /// Operation labels from newest to oldest ("history" panel).
    pub fn history(&self) -> Vec<&str> {
        self.undo
            .iter()
            .rev()
            .map(|entry| entry.label.as_str())
            .collect()
    }

    /// Full history (earliest → latest), including the redo stack: steps undone remain listed,
    /// marked `undone`.
    ///
    /// Unlike `history()`, this includes the redo stack: the engine's undo-history window keeps
    /// listing undone steps, clicking back is redo, rather than letting them vanish into thin air.
    pub fn history_entries(&self) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = self
            .undo
            .iter()
            .map(|entry| (entry.label.clone(), false))
            .collect();
        // The last element of the redo stack is the next step to redo, so reversing the whole
        // thing places it right after the undo steps in chronological order.
        out.extend(
            self.redo
                .iter()
                .rev()
                .map(|entry| (entry.label.clone(), true)),
        );
        out
    }

    /// Current position in `history_entries()` (= number of executed steps).
    pub fn history_cursor(&self) -> usize {
        self.undo.len()
    }

    pub fn clear(&mut self) {
        let was_dirty = self.is_dirty();
        self.undo.clear();
        self.redo.clear();
        self.current_state = self.allocate_state();
        // Clearing history must not silently wash the dirty flag: a clean document stays clean
        // after clearing history, but a dirty document stays dirty just because the history is gone.
        self.saved_state = if was_dirty {
            None
        } else {
            Some(self.current_state)
        };
    }

    /// A single field write. `entity_id + component + path` coalesces within the time window:
    /// the new `after` updates the previous step's `after` (keeping the original `before`).
    #[allow(clippy::too_many_arguments)]
    pub fn push_field(
        &mut self,
        doc: &mut SceneDoc,
        entity_id: u32,
        component: &str,
        path: &str,
        before: Value,
        after: Value,
        now_ms: u64,
    ) {
        if before == after {
            return;
        }
        let coalesce = format!("{entity_id}/{component}/{path}");
        let state = self.allocate_state();
        if let Some(last) = self.undo.last_mut() {
            if last.coalesce.as_deref() == Some(coalesce.as_str())
                && (self.sticky || now_ms.saturating_sub(last.at_ms) <= self.coalesce_ms)
            {
                if let Command::SetField {
                    after: last_after, ..
                } = &mut last.command
                {
                    *last_after = after.clone();
                    last.at_ms = now_ms;
                    last.after_state = state;
                    self.current_state = state;
                    self.redo.clear();
                    apply_field(doc, entity_id, component, path, &after);
                    return;
                }
            }
        }

        let command = Command::SetField {
            entity_id,
            component: component.to_string(),
            path: path.to_string(),
            before,
            after: after.clone(),
        };
        apply_field(doc, entity_id, component, path, &after);
        self.undo.push(Entry {
            label: format!("{} · {}", short_component(component), path),
            command,
            coalesce: Some(coalesce),
            at_ms: now_ms,
            selection: None,
            before_state: self.current_state,
            after_state: state,
        });
        self.current_state = state;
        self.redo.clear();
        self.trim();
    }

    /// A single Transform drag: consecutive edits of the same entity compress into one step.
    pub fn push_transform(
        &mut self,
        doc: &mut SceneDoc,
        entity_id: u32,
        before: TransformJson,
        after: TransformJson,
        now_ms: u64,
    ) {
        if before == after {
            return;
        }
        let coalesce = format!("transform/{entity_id}");
        let state = self.allocate_state();
        if let Some(last) = self.undo.last_mut() {
            if last.coalesce.as_deref() == Some(coalesce.as_str())
                && (self.sticky || now_ms.saturating_sub(last.at_ms) <= self.coalesce_ms)
            {
                if let Command::Transform {
                    after: last_after, ..
                } = &mut last.command
                {
                    *last_after = after;
                    last.at_ms = now_ms;
                    last.after_state = state;
                    self.current_state = state;
                    self.redo.clear();
                    apply_transform(doc, entity_id, &after);
                    return;
                }
            }
        }
        apply_transform(doc, entity_id, &after);
        self.undo.push(Entry {
            label: "Edit Transform".to_string(),
            command: Command::Transform {
                entity_id,
                before,
                after,
            },
            coalesce: Some(coalesce),
            at_ms: now_ms,
            selection: None,
            before_state: self.current_state,
            after_state: state,
        });
        self.current_state = state;
        self.redo.clear();
        self.trim();
    }

    /// Multi-select Transform drag: `changes` is `(entity_id, before, after)`,
    /// consecutive drags with the same `coalesce` key compress into one step (whole group undo together).
    pub fn push_transform_batch(
        &mut self,
        doc: &mut SceneDoc,
        changes: Vec<(u32, TransformJson, TransformJson)>,
        coalesce: String,
        now_ms: u64,
    ) {
        let changes: Vec<_> = changes
            .into_iter()
            .filter(|(_, before, after)| before != after)
            .collect();
        if changes.is_empty() {
            return;
        }
        let state = self.allocate_state();
        if let Some(last) = self.undo.last_mut() {
            if last.coalesce.as_deref() == Some(coalesce.as_str())
                && (self.sticky || now_ms.saturating_sub(last.at_ms) <= self.coalesce_ms)
            {
                if let Command::TransformBatch {
                    changes: last_changes,
                } = &mut last.command
                {
                    for (entity_id, before, after) in &changes {
                        match last_changes
                            .iter_mut()
                            .find(|(existing, _, _)| existing == entity_id)
                        {
                            Some(slot) => slot.2 = *after,
                            None => last_changes.push((*entity_id, *before, *after)),
                        }
                        apply_transform(doc, *entity_id, after);
                    }
                    last.at_ms = now_ms;
                    last.after_state = state;
                    self.current_state = state;
                    self.redo.clear();
                    return;
                }
            }
        }
        for (entity_id, _, after) in &changes {
            apply_transform(doc, *entity_id, after);
        }
        self.undo.push(Entry {
            label: "Edit Transform".to_string(),
            command: Command::TransformBatch { changes },
            coalesce: Some(coalesce),
            at_ms: now_ms,
            selection: None,
            before_state: self.current_state,
            after_state: state,
        });
        self.current_state = state;
        self.redo.clear();
        self.trim();
    }

    /// Structural edit: record `before` snapshot and edited document as one step.
    ///
    /// No selection snapshot — undo / redo leaves the selection as-is (caller corrects itself).
    pub fn push_snapshot(&mut self, label: impl Into<String>, before: SceneDoc, after: &SceneDoc) {
        self.push_snapshot_with_selection(label, before, None, after);
    }

    /// Structural edit + entity selection snapshot (#513).
    ///
    /// `selection` ids mapped back to indices on undo / redo by `EntityJson.id`; not found ids
    /// are simply dropped, so "delete then undo" accurately re-selects the original entity rather
    /// than whichever object happens to have the same index.
    pub fn push_snapshot_with_selection(
        &mut self,
        label: impl Into<String>,
        before: SceneDoc,
        selection: Option<SelectionSnapshot>,
        after: &SceneDoc,
    ) {
        if before == *after {
            return;
        }
        let state = self.allocate_state();
        self.undo.push(Entry {
            label: label.into(),
            command: Command::Snapshot {
                before: Box::new(before),
                after: Box::new(after.clone()),
            },
            coalesce: None,
            at_ms: 0,
            selection,
            before_state: self.current_state,
            after_state: state,
        });
        self.current_state = state;
        self.redo.clear();
        self.trim();
    }

    pub fn undo(&mut self, doc: &mut SceneDoc) -> Option<UndoOutcome> {
        let mut entry = self.undo.pop()?;
        match &entry.command {
            Command::SetField {
                entity_id,
                component,
                path,
                before,
                ..
            } => {
                apply_field(doc, *entity_id, component, path, before);
            }
            Command::Transform {
                entity_id, before, ..
            } => {
                apply_transform(doc, *entity_id, before);
            }
            Command::TransformBatch { changes } => {
                for (entity_id, before, _) in changes {
                    apply_transform(doc, *entity_id, before);
                }
            }
            Command::Snapshot { before, .. } => {
                *doc = (**before).clone();
            }
        }
        let selection = entry
            .selection
            .as_ref()
            .map(|snapshot| snapshot.before.clone());
        entry.coalesce = None;
        self.current_state = entry.before_state;
        let label = entry.label.clone();
        let transform_only = matches!(
            entry.command,
            Command::Transform { .. } | Command::TransformBatch { .. }
        );
        self.redo.push(entry);
        Some(UndoOutcome {
            label,
            selection,
            transform_only,
        })
    }

    pub fn redo(&mut self, doc: &mut SceneDoc) -> Option<UndoOutcome> {
        let mut entry = self.redo.pop()?;
        match &entry.command {
            Command::SetField {
                entity_id,
                component,
                path,
                after,
                ..
            } => {
                apply_field(doc, *entity_id, component, path, after);
            }
            Command::Transform {
                entity_id, after, ..
            } => {
                apply_transform(doc, *entity_id, after);
            }
            Command::TransformBatch { changes } => {
                for (entity_id, _, after) in changes {
                    apply_transform(doc, *entity_id, after);
                }
            }
            Command::Snapshot { after, .. } => {
                *doc = (**after).clone();
            }
        }
        let selection = entry
            .selection
            .as_ref()
            .map(|snapshot| snapshot.after.clone());
        entry.coalesce = None;
        self.current_state = entry.after_state;
        let label = entry.label.clone();
        let transform_only = matches!(
            entry.command,
            Command::Transform { .. } | Command::TransformBatch { .. }
        );
        self.undo.push(entry);
        Some(UndoOutcome {
            label,
            selection,
            transform_only,
        })
    }

    fn trim(&mut self) {
        let limit = self.limit;
        if self.undo.len() > limit {
            let excess = self.undo.len() - limit;
            self.undo.drain(0..excess);
        }
    }
}

fn apply_transform(doc: &mut SceneDoc, entity_id: u32, transform: &TransformJson) {
    if let Some(index) = doc.index_of_id(entity_id) {
        doc.entities[index].transform = *transform;
    }
}

fn short_component(component: &str) -> &str {
    component.rsplit("::").next().unwrap_or(component)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::SceneDoc;
    use serde_json::json;

    fn doc_with_camera() -> (SceneDoc, u32) {
        let mut doc = SceneDoc::default();
        let index = doc.add_named_child(None, "Camera");
        doc.add_component(
            index,
            "prism_engine::scene::Camera",
            json!({ "fov_y_degrees": 60.0, "near": 0.1 }),
        );
        let id = doc.entities[index].id.unwrap();
        (doc, id)
    }

    #[test]
    fn field_edits_coalesce_within_window() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(60.0),
            json!(61.0),
            0,
        );
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(61.0),
            json!(70.0),
            100,
        );
        assert_eq!(
            stack.history().len(),
            1,
            "continuous input on same field coalesces into one step"
        );
        assert_eq!(stack.history()[0], "Camera · fov_y_degrees");

        // Undo back to the initial 60.
        stack.undo(&mut doc);
        let camera = doc.entities[0]
            .components
            .get("prism_engine::scene::Camera")
            .unwrap();
        assert_eq!(camera["fov_y_degrees"], json!(60.0));
        assert!(stack.can_redo());

        stack.redo(&mut doc);
        let camera = doc.entities[0]
            .components
            .get("prism_engine::scene::Camera")
            .unwrap();
        assert_eq!(camera["fov_y_degrees"], json!(70.0));
    }

    #[test]
    fn different_fields_or_stale_edits_create_new_steps() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.2),
            0,
        );
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(60.0),
            json!(70.0),
            10,
        );
        assert_eq!(stack.history().len(), 2);

        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(70.0),
            json!(80.0),
            10_000,
        );
        assert_eq!(
            stack.history().len(),
            3,
            "exceeding the coalesce window counts as a new step"
        );
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.2),
            0,
        );
        stack.undo(&mut doc);
        assert!(stack.can_redo());
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.3),
            5_000,
        );
        assert!(!stack.can_redo(), "new edit clears redo");
    }

    #[test]
    fn snapshot_undo_redo_restores_whole_doc() {
        let (mut doc, _) = doc_with_camera();
        let before = doc.clone();
        doc.add_named_child(None, "Cube");
        let mut stack = UndoStack::new();
        stack.push_snapshot("new entity", before.clone(), &doc);
        assert_eq!(doc.entities.len(), 2);

        stack.undo(&mut doc);
        assert_eq!(doc.entities.len(), 1);
        stack.redo(&mut doc);
        assert_eq!(doc.entities.len(), 2);
    }

    #[test]
    fn equal_snapshots_are_ignored() {
        let (doc, _) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_snapshot("no change", doc.clone(), &doc);
        assert!(!stack.can_undo());
    }

    #[test]
    fn limit_drops_oldest_entries() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.set_limit(3);
        for step in 0..5 {
            stack.push_field(
                &mut doc,
                id,
                "prism_engine::scene::Camera",
                "near",
                json!(step as f64),
                json!(step as f64 + 1.0),
                step as u64 * 10_000,
            );
        }
        assert_eq!(stack.history().len(), 3);
    }

    #[test]
    fn transform_edits_coalesce_and_undo() {
        let (mut doc, id) = doc_with_camera();
        let before = doc.entities[0].transform;
        let mut after = before;
        after.position[0] = 5.0;
        let mut stack = UndoStack::new();
        stack.push_transform(&mut doc, id, before, after, 0);
        assert_eq!(doc.entities[0].transform.position[0], 5.0);

        let mut second = after;
        second.position[0] = 7.0;
        stack.push_transform(&mut doc, id, after, second, 100);
        assert_eq!(stack.history().len(), 1, "drag on same entity coalesces");
        assert_eq!(doc.entities[0].transform.position[0], 7.0);

        stack.undo(&mut doc);
        assert_eq!(doc.entities[0].transform.position[0], 0.0);
        stack.redo(&mut doc);
        assert_eq!(doc.entities[0].transform.position[0], 7.0);
    }

    #[test]
    fn interaction_boundary_holds_and_closes_undo_steps() {
        let (mut doc, id) = doc_with_camera();
        let before = doc.entities[0].transform;
        let mut stack = UndoStack::new();

        // During interaction: an 800ms+ pause still coalesces into one step.
        stack.begin_interaction();
        let mut step = before;
        step.position[0] = 1.0;
        stack.push_transform(&mut doc, id, before, step, 0);
        let mut step2 = step;
        step2.position[0] = 2.0;
        stack.push_transform(&mut doc, id, step, step2, 5_000);
        assert_eq!(stack.history().len(), 1, "one drag = one step");
        assert!(stack.interaction_active());

        // Release then drag again immediately: must open a new step even though interval < 800ms.
        stack.commit();
        assert!(!stack.interaction_active());
        let mut step3 = step2;
        step3.position[0] = 3.0;
        stack.push_transform(&mut doc, id, step2, step3, 5_010);
        assert_eq!(stack.history().len(), 2, "two drags = two steps");

        stack.undo(&mut doc);
        assert_eq!(doc.entities[0].transform.position[0], 2.0);
        stack.undo(&mut doc);
        assert_eq!(doc.entities[0].transform.position[0], 0.0);
    }

    #[test]
    fn missing_entity_is_silently_skipped() {
        let (mut doc, _) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(&mut doc, 999, "c", "p", json!(1), json!(2), 0);
        stack.undo(&mut doc);
        assert_eq!(doc.entities.len(), 1);
    }

    #[test]
    fn undo_back_to_the_saved_state_clears_dirty() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        assert!(!stack.is_dirty(), "new document is clean");

        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(60.0),
            json!(70.0),
            0,
        );
        assert!(stack.is_dirty());

        stack.undo(&mut doc);
        assert!(
            !stack.is_dirty(),
            "undoing back to saved point should be clean"
        );

        stack.redo(&mut doc);
        assert!(
            stack.is_dirty(),
            "redoing back away from saved point becomes dirty again"
        );
    }

    #[test]
    fn a_new_edit_at_the_saved_depth_is_dirty() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.2),
            0,
        );
        stack.undo(&mut doc);
        assert!(!stack.is_dirty());

        // Undoing then making a *different* edit at the same depth: stack length is 1 again,
        // but content differs from save point.
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.3),
            10_000,
        );
        assert!(
            stack.is_dirty(),
            "same depth but different content is still unsaved"
        );
    }

    #[test]
    fn mark_saved_moves_the_baseline() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(60.0),
            json!(70.0),
            0,
        );
        stack.mark_saved();
        assert!(!stack.is_dirty(), "after saving baseline follows forward");

        stack.undo(&mut doc);
        assert!(stack.is_dirty(), "undoing after saving is still unsaved");
        stack.redo(&mut doc);
        assert!(
            !stack.is_dirty(),
            "redoing back to saved point is clean again"
        );
    }

    #[test]
    fn clearing_history_keeps_the_dirty_state() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.clear();
        assert!(
            !stack.is_dirty(),
            "clean document clearing history stays clean"
        );

        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.2),
            0,
        );
        stack.clear();
        assert!(
            stack.is_dirty(),
            "clearing history cannot wash unsaved edits"
        );
    }
}
