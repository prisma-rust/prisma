//! Entity creation, deletion, clipboard, duplication, reparenting, and renaming.

use super::*;

pub(super) fn build_entity(
    schema: &Schema,
    preset: Option<&EntityPreset>,
    parent: Option<usize>,
    doc: &SceneDoc,
) -> EntityJson {
    let parent_id = parent.and_then(|index| doc.id_of_index(index));
    let (name, components, position, rotation) = match preset {
        Some(preset) => (
            preset.name.clone(),
            preset.components.clone(),
            preset.position,
            preset.rotation,
        ),
        None => (
            "GameObject".to_string(),
            Vec::new(),
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
        ),
    };
    let mut entity = EntityJson {
        name: Some(doc.unique_name(&name, parent_id)),
        parent: parent_id,
        transform: TransformJson {
            position,
            rotation,
            scale: [1.0; 3],
        },
        ..Default::default()
    };
    for component_name in components {
        let Some(component) = schema.component(&component_name) else {
            continue;
        };
        let override_value = preset.and_then(|preset| preset.overrides.get(&component_name));
        let value = component_value_with_overrides(&component.default_value(), override_value);
        entity.components.insert(component_name, value);
    }
    entity
}

impl Workspace {
    pub fn add_entity(&mut self, preset_id: &str) {
        let preset = self
            .presets
            .iter()
            .find(|preset| preset.id == preset_id)
            .cloned();
        let schema = self.schema.clone();
        let parent = self.primary_selection();
        self.edit("新建实体", |document| {
            let entity = build_entity(&schema, preset.as_ref(), parent, &document.doc);
            document.doc.add_entity(entity);
        });
    }

    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let indices = self.selection.clone();
        self.edit("删除实体", |document| {
            document.doc.remove_entities(&indices);
        });
        self.clear_selection();
    }

    pub fn copy_selection(&mut self) -> usize {
        let roots = self.selection_roots();
        if roots.is_empty() {
            return 0;
        }
        let trees: Vec<SceneDoc> = roots
            .iter()
            .map(|index| {
                self.document()
                    .doc
                    .extract_subtree_preserving_links(&[*index])
            })
            .collect();
        let count: usize = trees.iter().map(|tree| tree.entities.len()).sum();
        self.entity_clipboard = Some(EntityClipboard { trees, cut: false });
        self.log(
            LogSource::Editor,
            LogLevel::Debug,
            format!("已复制 {count} 个实体"),
        );
        count
    }

    pub fn cut_selection(&mut self) -> usize {
        let count = self.copy_selection();
        if count > 0 {
            if let Some(clipboard) = &mut self.entity_clipboard {
                clipboard.cut = true;
            }
            self.delete_selection();
        }
        count
    }

    pub fn paste_entities(&mut self, parent: Option<usize>) -> Vec<usize> {
        let Some(clipboard) = self.entity_clipboard.clone() else {
            return Vec::new();
        };
        if clipboard.trees.is_empty() {
            return Vec::new();
        }
        let parent_valid = parent.filter(|index| *index < self.document().doc.entities.len());
        let roots = self.edit("粘贴实体", |document| {
            let mut roots = Vec::new();
            for tree in &clipboard.trees {
                let inserted = document.doc.instantiate_prefab(tree, parent_valid, "");
                if let Some(root) = inserted.first() {
                    roots.push(*root);
                }
            }
            roots
        });
        if roots.is_empty() {
            return roots;
        }
        self.selection = roots.clone();
        self.selection_active = roots.last().copied();
        for index in &roots {
            if let Some(id) = self.document().doc.id_of_index(*index) {
                self.expanded.insert(id);
            }
        }
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("已粘贴 {} 个实体", roots.len()),
        );
        roots
    }

    pub fn duplicate_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let roots = self.selection_roots();
        let mut inserted = self.edit("复制实体", |document| {
            let mut inserted = Vec::new();

            for index in roots.iter().rev() {
                if let Some(new_index) = document.doc.duplicate_subtree(*index) {
                    inserted.push(new_index);
                }
            }
            inserted
        });
        if inserted.is_empty() {
            return;
        }
        inserted.sort_unstable();
        self.selection = inserted.clone();
        self.selection_active = inserted.last().copied();
        for index in &inserted {
            if let Some(id) = self.document().doc.id_of_index(*index) {
                self.expanded.insert(id);
            }
        }
    }

    pub fn reparent(&mut self, index: usize, new_parent: Option<usize>, position: Option<usize>) {
        let result = self.edit("调整层级", |document| {
            document.doc.reparent(index, new_parent, position).is_ok()
        });
        if !result {
            self.log(
                LogSource::Editor,
                LogLevel::Warn,
                "层级调整被拒绝（自父 / 成环）",
            );
        }
    }

    pub fn reparent_selection(&mut self, new_parent: Option<usize>) -> usize {
        let roots = self.selection_roots();
        if roots.is_empty() {
            return 0;
        }
        let result = self.edit("调整层级", |document| {
            document.doc.reparent_many(&roots, new_parent)
        });
        match result {
            Ok(moved) => moved,
            Err(reason) => {
                self.log(LogSource::Editor, LogLevel::Warn, reason);
                0
            }
        }
    }

    pub fn reparent_selection_at(&mut self, target: usize, before: bool) -> usize {
        let roots = self.selection_roots();
        if roots.is_empty() {
            return 0;
        }
        let result = self.edit("调整层级", |document| {
            document.doc.reparent_many_at(&roots, target, before)
        });
        match result {
            Ok(moved) => moved,
            Err(reason) => {
                self.log(LogSource::Editor, LogLevel::Warn, reason);
                0
            }
        }
    }

    pub fn set_active_entity(&mut self, index: usize, active: bool) {
        self.edit(
            if active {
                "启用实体"
            } else {
                "禁用实体"
            },
            |document| {
                document.doc.set_active(index, active);
            },
        );
    }

    pub fn preview_rename(&mut self, index: usize, name: &str) {
        if let Some(entity) = self.document_mut().doc.entities.get_mut(index) {
            if entity.name.as_deref() != Some(name) {
                entity.name = Some(name.to_string());
                self.document_mut().dirty = true;
            }
        }
    }

    pub fn finish_rename(&mut self, base: SceneDoc, was_dirty: bool) {
        let after = self.document().doc.clone();
        if base == after {
            self.document_mut().dirty = was_dirty;
            return;
        }
        let document = self.document_mut();
        document.undo.push_snapshot("重命名实体", base, &after);
        document.dirty = was_dirty;
        document.mark_dirty();
    }

    pub fn cancel_rename(&mut self, base: SceneDoc, was_dirty: bool) {
        let document = self.document_mut();
        document.doc = base;
        document.dirty = was_dirty;
        document.mark_dirty();
    }

    pub fn rename_entity(&mut self, index: usize, name: &str) {
        self.edit("重命名实体", |document| {
            document.doc.rename(index, name);
        });
    }
}
