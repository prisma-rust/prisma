//! Central command bus and reversible undo/redo stack.
//!
//! Every state mutation in the editor flows through [`EditorCommand`], keeping
//! menus, toolbars, panels, shortcuts, and tests on a single code path.

use serde_json::Value;

use prism_ui::prelude::{Locale, ThemePreset, WindowId};

use crate::inspector::FieldEdit;
use crate::state::{EditorDocument, EditorEntity, GizmoMode};

/// A document mutation that knows how to apply and revert itself.
pub trait ReversibleCommand {
    /// Short human-readable label describing this command.
    fn label(&self) -> &'static str;
    /// Apply the forward mutation to `doc`.
    fn apply(&self, doc: &mut EditorDocument);
    /// Revert the mutation on `doc`.
    fn revert(&self, doc: &mut EditorDocument);
}

/// Reversible document-level commands managed by [`UndoStack`].
#[derive(Clone, Debug, PartialEq)]
pub enum DocCommand {
    SetField(FieldEdit),
    CreateEntity {
        id: u32,
        name: String,
        parent: Option<u32>,
    },
    RenameEntity {
        id: u32,
        old_name: String,
        new_name: String,
    },
}

impl ReversibleCommand for DocCommand {
    fn label(&self) -> &'static str {
        match self {
            Self::SetField(_) => "Edit Field",
            Self::CreateEntity { .. } => "Create Entity",
            Self::RenameEntity { .. } => "Rename Entity",
        }
    }

    fn apply(&self, doc: &mut EditorDocument) {
        match self {
            Self::SetField(edit) => {
                doc.apply_field_value(
                    edit.entity_id,
                    &edit.component,
                    &edit.field,
                    edit.axis,
                    edit.new_value.clone(),
                );
            }
            Self::CreateEntity { id, name, parent } => {
                if !doc.entities.iter().any(|e| e.id == *id) {
                    doc.entities.push(EditorEntity {
                        id: *id,
                        name: name.clone(),
                        parent: *parent,
                        components: Default::default(),
                    });
                    if let Some(p) = parent {
                        doc.expanded.insert(*p);
                    }
                    doc.selected_entity = *id;
                    doc.dirty = true;
                }
            }
            Self::RenameEntity { id, new_name, .. } => {
                if let Some(ent) = doc.entities.iter_mut().find(|e| e.id == *id) {
                    ent.name = new_name.clone();
                    doc.dirty = true;
                }
            }
        }
    }

    fn revert(&self, doc: &mut EditorDocument) {
        match self {
            Self::SetField(edit) => {
                doc.apply_field_value(
                    edit.entity_id,
                    &edit.component,
                    &edit.field,
                    edit.axis,
                    edit.old_value.clone(),
                );
            }
            Self::CreateEntity { id, .. } => {
                doc.entities.retain(|e| e.id != *id);
                if doc.selected_entity == *id {
                    doc.selected_entity = doc.entities.first().map(|e| e.id).unwrap_or(0);
                }
                doc.dirty = true;
            }
            Self::RenameEntity { id, old_name, .. } => {
                if let Some(ent) = doc.entities.iter_mut().find(|e| e.id == *id) {
                    ent.name = old_name.clone();
                    doc.dirty = true;
                }
            }
        }
    }
}

/// Bounded-free undo/redo stack for [`DocCommand`].
#[derive(Default)]
pub struct UndoStack {
    undo: Vec<DocCommand>,
    redo: Vec<DocCommand>,
}

impl UndoStack {
    /// Record an already-applied or newly-applied command; clears redo history.
    pub fn push(&mut self, cmd: DocCommand) {
        if let DocCommand::SetField(ref edit) = cmd {
            if edit.old_value == edit.new_value {
                return;
            }
        }
        self.undo.push(cmd);
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label of the next command to undo, if any.
    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(ReversibleCommand::label)
    }

    /// Label of the next command to redo, if any.
    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(ReversibleCommand::label)
    }

    pub fn pop_undo(&mut self) -> Option<DocCommand> {
        let cmd = self.undo.pop()?;
        self.redo.push(cmd.clone());
        Some(cmd)
    }

    pub fn pop_redo(&mut self) -> Option<DocCommand> {
        let cmd = self.redo.pop()?;
        self.undo.push(cmd.clone());
        Some(cmd)
    }
}

/// Unified command enum for all editor interactions.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorCommand {
    /// Apply a reversible document mutation and push it onto the undo stack.
    Doc(DocCommand),
    /// Live-preview a field change during a drag without pushing an undo entry.
    PreviewField {
        entity_id: u32,
        component: String,
        field: String,
        axis: Option<usize>,
        value: Value,
    },
    Undo,
    Redo,
    SelectEntity(u32),
    SetEntityExpanded {
        id: u32,
        expanded: bool,
    },
    SetSearchQuery(String),
    SetGizmoMode(GizmoMode),
    SetPlaying(bool),
    TogglePlay,
    ToggleGrid,
    ToggleGizmos,
    SetTheme(ThemePreset),
    SetLocale(Locale),
    ToggleWindow(WindowId),
    OpenAbout,
    CloseAbout,
    ResetLayout,
}
