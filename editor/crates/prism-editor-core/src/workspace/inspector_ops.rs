//! Component editing, UI anchor presets, and component presets.

use super::*;

impl Workspace {
    pub fn component_types(&self) -> Vec<&TypeSchema> {
        let indices: Vec<usize> = self.primary_selection().into_iter().collect();
        self.component_types_for(&indices)
    }

    pub fn component_types_for(&self, indices: &[usize]) -> Vec<&TypeSchema> {
        let entities = &self.document().doc.entities;
        self.schema
            .components()
            .into_iter()
            .filter(|component| {
                indices.iter().any(|index| {
                    entities
                        .get(*index)
                        .map(|entity| !entity.has_component(&component.name))
                        .unwrap_or(false)
                })
            })
            .collect()
    }

    pub fn add_component(&mut self, component_name: &str) {
        let indices: Vec<usize> = self.primary_selection().into_iter().collect();
        self.add_component_multi(&indices, component_name);
    }

    pub fn add_component_multi(&mut self, indices: &[usize], component_name: &str) -> usize {
        let Some(component) = self.schema.component(component_name) else {
            return 0;
        };
        let mut targets: Vec<usize> = indices.to_vec();
        targets.sort_unstable();
        targets.dedup();
        let entities = &self.document().doc.entities;
        let missing: Vec<usize> = targets
            .into_iter()
            .filter(|index| {
                entities
                    .get(*index)
                    .map(|entity| !entity.has_component(component_name))
                    .unwrap_or(false)
            })
            .collect();
        if missing.is_empty() {
            if !indices.is_empty() {
                self.log(
                    LogSource::Editor,
                    LogLevel::Warn,
                    format!("{} 已有该组件，未做修改", component.display_name()),
                );
            }
            return 0;
        }
        let name = component.name.clone();
        let value = component_value_with_overrides(&component.default_value(), None);
        let added = self.edit("添加组件", |document| {
            for index in &missing {
                let entity = &mut document.doc.entities[*index];
                entity.components.insert(name.clone(), value.clone());
            }
            missing.len()
        });
        added
    }

    pub fn remove_component(&mut self, component_name: &str) {
        let Some(index) = self.primary_selection() else {
            return;
        };
        if component_name == "Transform" {
            return;
        }
        self.edit("移除组件", |document| {
            document.doc.remove_component(index, component_name);
        });
    }

    pub fn move_component_at(&mut self, index: usize, component_name: &str, delta: isize) -> bool {
        if component_name == "Transform" || delta == 0 {
            return false;
        }
        self.edit("调整组件顺序", |document| {
            let Some(entity) = document.doc.entities.get_mut(index) else {
                return false;
            };
            entity.components.move_by(component_name, delta)
        })
    }

    pub fn move_component(&mut self, component_name: &str, delta: isize) -> bool {
        let Some(index) = self.primary_selection() else {
            return false;
        };
        self.move_component_at(index, component_name, delta)
    }

    pub fn component_order(&self, index: usize) -> Vec<String> {
        self.document()
            .doc
            .entities
            .get(index)
            .map(|entity| entity.components.order())
            .unwrap_or_default()
    }

    pub fn copy_component(&mut self, component_name: &str) {
        let Some(index) = self.primary_selection() else {
            return;
        };
        self.copy_component_at(index, component_name);
    }

    pub fn copy_component_at(&mut self, index: usize, component_name: &str) {
        let Some(value) = self.document().doc.entities[index].component(component_name) else {
            return;
        };
        self.clipboard = Some(ComponentClipboard {
            component: component_name.to_string(),
            value: value.clone(),
        });
        self.log(
            LogSource::Editor,
            LogLevel::Debug,
            format!("已复制组件 {component_name}"),
        );
    }

    pub fn paste_component(&mut self) {
        let Some(clipboard) = self.clipboard.clone() else {
            return;
        };
        let Some(index) = self.primary_selection() else {
            return;
        };
        self.edit("粘贴组件", |document| {
            document.doc.entities[index]
                .components
                .insert(clipboard.component.clone(), clipboard.value.clone());
        });
    }

    pub fn paste_component_values(&mut self, indices: &[usize], component_name: &str) -> usize {
        let Some(clipboard) = self.clipboard.clone() else {
            return 0;
        };
        if clipboard.component != component_name {
            return 0;
        }
        let indices = indices.to_vec();
        let component_name = component_name.to_string();
        self.edit("粘贴组件值", |document| {
            let mut applied = 0;
            for index in indices {
                let Some(component) = document
                    .doc
                    .entities
                    .get_mut(index)
                    .and_then(|entity| entity.components.get_mut(&component_name))
                else {
                    continue;
                };
                *component = clipboard.value.clone();
                applied += 1;
            }
            applied
        })
    }

    pub fn common_components(&self, indices: &[usize]) -> Vec<TypeSchema> {
        let doc = &self.document().doc;
        let first = indices
            .iter()
            .filter_map(|index| doc.entities.get(*index))
            .next();
        let Some(first) = first else {
            return Vec::new();
        };
        let mut common: Vec<TypeSchema> = first
            .components
            .keys()
            .filter_map(|name| self.schema.component(name).cloned())
            .collect();
        for index in indices.iter().skip(1) {
            let Some(entity) = doc.entities.get(*index) else {
                continue;
            };
            common.retain(|component| entity.has_component(&component.name));
        }
        common
    }

    pub fn set_field_multi(
        &mut self,
        entity_ids: &[u32],
        component: &str,
        path: &str,
        value: Value,
    ) -> usize {
        if entity_ids.is_empty() {
            return 0;
        }
        if entity_ids.len() == 1 {
            self.set_field(entity_ids[0], component, path, value);
            return 1;
        }
        let component_name = component.to_string();
        let path = path.to_string();
        self.edit("批量修改字段", |document| {
            let mut applied = 0;
            for id in entity_ids {
                let Some(index) = document.doc.index_of_id(*id) else {
                    continue;
                };
                let Some(root) = document.doc.entities[index]
                    .components
                    .get_mut(&component_name)
                else {
                    continue;
                };
                if crate::schema::set_at_path(root, &path, value.clone()) {
                    applied += 1;
                }
            }
            applied
        })
    }

    pub fn remove_component_multi(&mut self, indices: &[usize], component: &str) {
        let indices = indices.to_vec();
        let component_name = component.to_string();
        self.edit("批量移除组件", |document| {
            for index in &indices {
                document.doc.remove_component(*index, &component_name);
            }
        });
    }

    pub fn set_field(&mut self, entity_id: u32, component: &str, path: &str, value: Value) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let Some(root) = self.document().doc.entities[index]
            .components
            .get(component)
        else {
            return;
        };
        let Some(before) = crate::schema::get_at(root, path) else {
            return;
        };
        if before == value {
            return;
        }
        let now_ms = now_ms();
        let document = self.document_mut();
        document.undo.push_field(
            &mut document.doc,
            entity_id,
            component,
            path,
            before,
            value,
            now_ms,
        );
        document.mark_dirty();
    }

    pub fn set_ui_anchors_preserving_rect(
        &mut self,
        index: usize,
        anchors: [f32; 4],
        parent: [f32; 4],
        rect: [f32; 4],
    ) -> bool {
        const STYLE: &str = "prism_engine::ui::Style";
        if parent[2] <= 0.0 || parent[3] <= 0.0 {
            return false;
        }
        let [min_x, min_y, max_x, max_y] = anchors.map(|value| value.clamp(0.0, 1.0));
        self.edit("调整 UI 锚点", |document| {
            let Some(style) = document
                .doc
                .entities
                .get_mut(index)
                .and_then(|entity| entity.components.get_mut(STYLE))
            else {
                return false;
            };
            if !style.is_object() {
                *style = serde_json::json!({});
            }
            let object = style.as_object_mut().expect("style object");

            let width = object
                .get("width")
                .and_then(Value::as_f64)
                .map(|value| value as f32);
            let height = object
                .get("height")
                .and_then(Value::as_f64)
                .map(|value| value as f32);
            let pivot = |axis: &str| {
                object
                    .get("pivot")
                    .and_then(|pivot| pivot.get(axis))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
            };
            let (pivot_x, pivot_y) = (pivot("x"), pivot("y"));

            let anchor_left = parent[0] + min_x * parent[2];
            let anchor_right = parent[0] + max_x * parent[2];
            let anchor_top = parent[1] + min_y * parent[3];
            let anchor_bottom = parent[1] + max_y * parent[3];
            let stretched_x = (max_x - min_x).abs() > 1e-6;
            let stretched_y = (max_y - min_y).abs() > 1e-6;

            let (margin_left, margin_right, new_width) = if stretched_x {
                (
                    rect[0] - anchor_left,
                    anchor_right - (rect[0] + rect[2]),
                    width,
                )
            } else {
                let width = width.unwrap_or(rect[2]);
                (rect[0] - anchor_left + pivot_x * width, 0.0, Some(width))
            };
            let (margin_top, margin_bottom, new_height) = if stretched_y {
                (
                    rect[1] - anchor_top,
                    anchor_bottom - (rect[1] + rect[3]),
                    height,
                )
            } else {
                let height = height.unwrap_or(rect[3]);
                (rect[1] - anchor_top + pivot_y * height, 0.0, Some(height))
            };

            if let Some(width) = new_width {
                object.insert("width".to_string(), serde_json::json!(width));
            }
            if let Some(height) = new_height {
                object.insert("height".to_string(), serde_json::json!(height));
            }

            let anchors_value = object
                .entry("anchors".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !anchors_value.is_object() {
                *anchors_value = serde_json::json!({});
            }
            let anchors_value = anchors_value.as_object_mut().expect("anchors object");
            anchors_value.insert("min_x".to_string(), serde_json::json!(min_x));
            anchors_value.insert("min_y".to_string(), serde_json::json!(min_y));
            anchors_value.insert("max_x".to_string(), serde_json::json!(max_x));
            anchors_value.insert("max_y".to_string(), serde_json::json!(max_y));

            let margin = object
                .entry("margin".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !margin.is_object() {
                *margin = serde_json::json!({});
            }
            let margin = margin.as_object_mut().expect("margin object");
            margin.insert("left".to_string(), serde_json::json!(margin_left));
            margin.insert("right".to_string(), serde_json::json!(margin_right));
            margin.insert("top".to_string(), serde_json::json!(margin_top));
            margin.insert("bottom".to_string(), serde_json::json!(margin_bottom));
            true
        })
    }

    pub fn set_ui_anchors_raw(&mut self, index: usize, anchors: [f32; 4]) -> bool {
        const STYLE: &str = "prism_engine::ui::Style";
        let [min_x, min_y, max_x, max_y] = anchors.map(|value| value.clamp(0.0, 1.0));
        self.edit("调整 UI 锚点（Raw）", |document| {
            let Some(style) = document
                .doc
                .entities
                .get_mut(index)
                .and_then(|entity| entity.components.get_mut(STYLE))
            else {
                return false;
            };
            if !style.is_object() {
                *style = serde_json::json!({});
            }
            let object = style.as_object_mut().expect("style object");
            let anchors_value = object
                .entry("anchors".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !anchors_value.is_object() {
                *anchors_value = serde_json::json!({});
            }
            let anchors_value = anchors_value.as_object_mut().expect("anchors object");
            anchors_value.insert("min_x".to_string(), serde_json::json!(min_x));
            anchors_value.insert("min_y".to_string(), serde_json::json!(min_y));
            anchors_value.insert("max_x".to_string(), serde_json::json!(max_x));
            anchors_value.insert("max_y".to_string(), serde_json::json!(max_y));
            true
        })
    }

    pub fn component_preset_names(&self, component: &str) -> Vec<String> {
        self.component_presets
            .iter()
            .filter(|preset| preset.component == component)
            .map(|preset| preset.name.clone())
            .collect()
    }

    pub fn component_preset_value(&self, component: &str, name: &str) -> Option<Value> {
        self.component_presets
            .iter()
            .find(|preset| preset.component == component && preset.name == name)
            .map(|preset| preset.value.clone())
    }

    pub fn save_component_preset(&mut self, component: &str, name: &str, value: Value) {
        let name = name.trim();
        if component.is_empty() || name.is_empty() {
            return;
        }
        if let Some(preset) = self
            .component_presets
            .iter_mut()
            .find(|preset| preset.component == component && preset.name == name)
        {
            preset.value = value;
            return;
        }
        self.component_presets.push(ComponentPreset {
            component: component.to_string(),
            name: name.to_string(),
            value,
        });
    }

    pub fn component_value(&self, entity: usize, component: &str) -> Option<&Value> {
        self.document()
            .doc
            .entities
            .get(entity)?
            .components
            .get(component)
    }
}
