//! Entity selection, hierarchy expansion, and Scene view visibility.

use super::*;

pub(super) fn selection_ids_in(doc: &crate::doc::SceneDoc, selection: &[usize]) -> Vec<u32> {
    selection
        .iter()
        .filter_map(|index| doc.entities.get(*index).and_then(|entity| entity.id))
        .collect()
}

impl Workspace {
    pub fn selection_ids(&self) -> Vec<u32> {
        selection_ids_in(&self.document().doc, &self.selection)
    }

    pub fn select_ids(&mut self, ids: &[u32]) -> usize {
        let document = self.document();
        let mut selection: Vec<usize> = ids
            .iter()
            .filter_map(|id| document.doc.index_of_id(*id))
            .collect();
        selection.sort_unstable();
        selection.dedup();
        let count = selection.len();
        self.selection = selection;
        self.selection_active = self.selection.last().copied();
        count
    }

    pub(super) fn clamp_selection(&mut self) {
        let len = self.document().doc.entities.len();
        self.selection.retain(|index| *index < len);
        self.selection.sort_unstable();
        self.selection.dedup();
        self.normalize_selection_active();
    }

    pub fn select(&mut self, index: usize, extend: bool) {
        if !extend {
            self.clear_selection();
        }
        if let Some(position) = self.selection.iter().position(|item| *item == index) {
            if extend {
                self.selection.remove(position);
            }
        } else {
            self.selection.push(index);
        }
        self.selection.sort_unstable();
        if self.selection.contains(&index) {
            self.selection_active = Some(index);
        } else {
            self.normalize_selection_active();
        }
    }

    pub fn select_for_context(&mut self, index: usize) -> bool {
        if self.selection.contains(&index) {
            self.selection_active = Some(index);
            return false;
        }
        if index >= self.document().doc.entities.len() {
            return false;
        }
        self.select_with(index, SelectMode::Replace);
        true
    }

    pub fn select_with(&mut self, index: usize, mode: SelectMode) {
        match mode {
            SelectMode::Replace => {
                self.clear_selection();
                self.selection.push(index);
                self.selection_active = Some(index);
            }
            SelectMode::Add => {
                if !self.selection.contains(&index) {
                    self.selection.push(index);
                }
                self.selection_active = Some(index);
            }
            SelectMode::Remove => {
                self.selection.retain(|item| *item != index);
                if self.selection_active == Some(index) {
                    self.selection_active = None;
                }
                self.normalize_selection_active();
            }
        }
        self.selection.sort_unstable();
    }

    pub fn select_range(&mut self, anchor: usize, index: usize) {
        let (start, end) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        self.selection = (start..=end).collect();
        self.selection_active = Some(index);
    }

    pub fn select_visible_range(&mut self, ordered: &[usize], anchor: usize, index: usize) {
        let Some(anchor_pos) = ordered.iter().position(|row| *row == anchor) else {
            self.select(index, false);
            return;
        };
        let Some(target_pos) = ordered.iter().position(|row| *row == index) else {
            self.select(index, false);
            return;
        };
        let (start, end) = if anchor_pos <= target_pos {
            (anchor_pos, target_pos)
        } else {
            (target_pos, anchor_pos)
        };
        self.selection = ordered[start..=end].to_vec();
        self.selection.sort_unstable();
        self.selection_active = Some(index);
    }

    pub fn select_many(&mut self, indices: &[usize], additive: bool) {
        self.select_many_with(
            indices,
            if additive {
                SelectMode::Add
            } else {
                SelectMode::Replace
            },
        );
    }

    pub fn select_many_with(&mut self, indices: &[usize], mode: SelectMode) {
        match mode {
            SelectMode::Replace => {
                self.clear_selection();
                for index in indices {
                    if !self.selection.contains(index) {
                        self.selection.push(*index);
                    }
                }
                self.selection_active = indices.last().copied();
            }
            SelectMode::Add => {
                for index in indices {
                    if !self.selection.contains(index) {
                        self.selection.push(*index);
                    }
                }
                if let Some(index) = indices.last() {
                    self.selection_active = Some(*index);
                }
            }
            SelectMode::Remove => {
                self.selection.retain(|index| !indices.contains(index));
                if self
                    .selection_active
                    .is_some_and(|index| indices.contains(&index))
                {
                    self.selection_active = None;
                }
                self.normalize_selection_active();
            }
        }
        self.selection.sort_unstable();
    }

    pub fn clear_selection_in(&mut self, mode: SelectMode) {
        if mode == SelectMode::Replace {
            self.selection.clear();
            self.selection_active = None;
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.selection_active = None;
    }

    pub fn set_entity_hidden(&mut self, index: usize, hidden: bool) {
        let ids: Vec<u32> = {
            let doc = &self.document().doc;
            std::iter::once(index)
                .chain(doc.descendants(index))
                .filter_map(|item| doc.id_of_index(item))
                .collect()
        };
        let set = &mut self.document_mut().hidden_entities;
        for id in ids {
            if hidden {
                set.insert(id);
            } else {
                set.remove(&id);
            }
        }
    }

    pub fn show_all_entities(&mut self) {
        self.document_mut().hidden_entities.clear();
        self.isolate = false;
    }

    pub fn set_isolate(&mut self, isolate: bool) {
        if isolate && self.selection.is_empty() {
            return;
        }
        self.isolate = isolate;
    }

    pub fn hidden_entity_ids(&self) -> Vec<u32> {
        let document = self.document();
        let doc = &document.doc;
        let isolate_visible: Option<HashSet<u32>> = self.isolate.then(|| {
            self.selection
                .iter()
                .flat_map(|root| std::iter::once(*root).chain(doc.descendants(*root)))
                .filter_map(|index| doc.id_of_index(index))
                .collect()
        });
        doc.entities
            .iter()
            .filter_map(|entity| entity.id)
            .filter(|id| {
                document.hidden_entities.contains(id)
                    || isolate_visible
                        .as_ref()
                        .is_some_and(|visible| !visible.contains(id))
            })
            .collect()
    }

    pub fn is_entity_hidden(&self, index: usize) -> bool {
        let document = self.document();
        let doc = &document.doc;
        let Some(id) = doc.id_of_index(index) else {
            return false;
        };
        if document.hidden_entities.contains(&id) {
            return true;
        }
        if !self.isolate {
            return false;
        }
        !self
            .selection
            .iter()
            .flat_map(|root| std::iter::once(*root).chain(doc.descendants(*root)))
            .filter_map(|item| doc.id_of_index(item))
            .any(|visible| visible == id)
    }

    pub fn primary_selection(&self) -> Option<usize> {
        self.selection_active
            .filter(|index| self.selection.contains(index))
            .or_else(|| self.selection.last().copied())
    }

    pub(super) fn normalize_selection_active(&mut self) {
        if self
            .selection_active
            .is_some_and(|index| !self.selection.contains(&index))
        {
            self.selection_active = self.selection.last().copied();
        }
    }

    pub fn toggle_expanded(&mut self, index: usize) {
        if let Some(id) = self.document().doc.id_of_index(index) {
            if !self.expanded.insert(id) {
                self.expanded.remove(&id);
            }
        }
    }

    pub fn expand_all(&mut self) {
        let ids: Vec<u32> = self
            .document()
            .doc
            .entities
            .iter()
            .filter_map(|entity| entity.id)
            .collect();
        self.expanded.extend(ids);
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    pub fn set_subtree_expanded(&mut self, index: usize, expanded: bool) {
        let doc = &self.document().doc;
        let mut indices = vec![index];
        indices.extend(doc.descendants(index));
        let ids: Vec<u32> = indices
            .iter()
            .filter_map(|index| doc.id_of_index(*index))
            .collect();
        for id in ids {
            if expanded {
                self.expanded.insert(id);
            } else {
                self.expanded.remove(&id);
            }
        }
    }

    pub fn selection_roots(&self) -> Vec<usize> {
        Self::roots_of(&self.document().doc, &self.selection)
    }

    pub(super) fn roots_of(doc: &SceneDoc, selection: &[usize]) -> Vec<usize> {
        let ids: HashSet<u32> = selection
            .iter()
            .filter_map(|index| doc.entities.get(*index).and_then(|entity| entity.id))
            .collect();
        let mut roots = Vec::new();
        for index in selection {
            let Some(entity) = doc.entities.get(*index) else {
                continue;
            };
            let mut parent = entity.parent;
            let mut nested = false;
            let mut depth = 0;
            while let Some(parent_id) = parent {
                if ids.contains(&parent_id) {
                    nested = true;
                    break;
                }
                depth += 1;
                if depth > 64 {
                    break;
                }
                parent = doc
                    .index_of_id(parent_id)
                    .and_then(|parent_index| doc.entities.get(parent_index))
                    .and_then(|parent| parent.parent);
            }
            if !nested {
                roots.push(*index);
            }
        }
        roots.sort_unstable();
        roots.dedup();
        roots
    }

    pub(super) fn prune_selected_descendants(doc: &SceneDoc, ids: &[u32]) -> Vec<u32> {
        let selected: HashSet<u32> = ids.iter().copied().collect();
        ids.iter()
            .copied()
            .filter(|id| {
                let Some(index) = doc.index_of_id(*id) else {
                    return true;
                };
                let mut parent = doc.entities[index].parent;
                while let Some(parent_id) = parent {
                    if selected.contains(&parent_id) {
                        return false;
                    }
                    parent = doc
                        .index_of_id(parent_id)
                        .and_then(|parent_index| doc.entities[parent_index].parent);
                }
                true
            })
            .collect()
    }

    pub fn gizmo_center_world(&self) -> Option<[f32; 3]> {
        let doc = &self.document().doc;
        match self.pivot_mode {
            PivotMode::Pivot => self
                .primary_selection()
                .and_then(|index| doc.entities.get(index).map(|_| world_position(doc, index)))
                .or_else(|| self.selection_center_world()),
            PivotMode::Center => self.selection_center_world(),
        }
    }

    pub fn selection_entity_ids(&self) -> Vec<u32> {
        let doc = &self.document().doc;
        self.selection
            .iter()
            .filter_map(|index| doc.id_of_index(*index))
            .collect()
    }

    pub fn selection_center_world(&self) -> Option<[f32; 3]> {
        let doc = &self.document().doc;
        let mut center = [0.0; 3];
        let mut count = 0;
        for index in &self.selection {
            let position = world_position(doc, *index);
            center[0] += position[0];
            center[1] += position[1];
            center[2] += position[2];
            count += 1;
        }
        (count > 0).then(|| {
            [
                center[0] / count as f32,
                center[1] / count as f32,
                center[2] / count as f32,
            ]
        })
    }
}
