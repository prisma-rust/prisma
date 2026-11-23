//! Prefab instantiation, overrides, revert, apply, and unpacking.

use super::*;

pub(super) fn root_id_of(workspace: &Workspace, index: usize) -> io::Result<u32> {
    workspace
        .document()
        .doc
        .id_of_index(index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "entity has no id"))
}

pub(super) fn json_vec3(value: &Value) -> Option<[f32; 3]> {
    let items = value.as_array()?;
    if items.len() != 3 {
        return None;
    }
    Some([
        items[0].as_f64()? as f32,
        items[1].as_f64()? as f32,
        items[2].as_f64()? as f32,
    ])
}

pub(super) fn json_quat(value: &Value) -> Option<[f32; 4]> {
    let items = value.as_array()?;
    if items.len() != 4 {
        return None;
    }
    Some([
        items[0].as_f64()? as f32,
        items[1].as_f64()? as f32,
        items[2].as_f64()? as f32,
        items[3].as_f64()? as f32,
    ])
}

pub(super) fn set_json_field(component: &mut Value, field: &str, value: Value) {
    if let Some(map) = component.as_object_mut() {
        if value.is_null() {
            map.remove(field);
        } else {
            map.insert(field.to_string(), value);
        }
    }
}

pub(super) fn values_close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
            (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| values_close(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| values_close(value, other)))
        }
        _ => a == b,
    }
}

pub(super) fn overrides_between(
    instance: &EntityJson,
    instance_index: usize,
    source: &EntityJson,
    _source_index: usize,
) -> Vec<PrefabOverride> {
    let mut out = Vec::new();
    let name = instance.display_name();
    for (field, left, right) in [
        (
            "position",
            serde_json::json!(instance.transform.position),
            serde_json::json!(source.transform.position),
        ),
        (
            "rotation",
            serde_json::json!(instance.transform.rotation),
            serde_json::json!(source.transform.rotation),
        ),
        (
            "scale",
            serde_json::json!(instance.transform.scale),
            serde_json::json!(source.transform.scale),
        ),
    ] {
        if !values_close(&left, &right) {
            out.push(PrefabOverride {
                entity: instance_index,
                entity_name: name.clone(),
                component: "transform".to_string(),
                field: field.to_string(),
                instance_value: left,
                prefab_value: right,
            });
        }
    }

    let mut components: Vec<&String> = instance
        .components
        .keys()
        .chain(source.components.keys())
        .filter(|name| name.as_str() != PREFAB_LINK_COMPONENT)
        .collect();
    components.sort();
    components.dedup();
    for component in components {
        let left = instance.components.get(component);
        let right = source.components.get(component);
        let mut fields: Vec<String> = left
            .and_then(|value| value.as_object())
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(map) = right.and_then(|value| value.as_object()) {
            fields.extend(map.keys().cloned());
        }
        fields.sort();
        fields.dedup();
        if fields.is_empty() {
            let (left, right) = (
                left.cloned().unwrap_or(Value::Null),
                right.cloned().unwrap_or(Value::Null),
            );
            if !values_close(&left, &right) {
                out.push(PrefabOverride {
                    entity: instance_index,
                    entity_name: name.clone(),
                    component: component.clone(),
                    field: String::new(),
                    instance_value: left,
                    prefab_value: right,
                });
            }
            continue;
        }
        for field in fields {
            let left = left
                .and_then(|value| value.get(&field))
                .cloned()
                .unwrap_or(Value::Null);
            let right = right
                .and_then(|value| value.get(&field))
                .cloned()
                .unwrap_or(Value::Null);
            if !values_close(&left, &right) {
                out.push(PrefabOverride {
                    entity: instance_index,
                    entity_name: name.clone(),
                    component: component.clone(),
                    field,
                    instance_value: left,
                    prefab_value: right,
                });
            }
        }
    }
    out
}

impl Workspace {
    pub fn instantiate_prefab_asset(
        &mut self,
        relative: &str,
        parent: Option<usize>,
    ) -> io::Result<usize> {
        let prefab = match &self.project_root {
            Some(root) if root.join(relative).exists() => project::read_document(root, relative)?,
            _ => {
                let stem = relative
                    .rsplit('/')
                    .next()
                    .unwrap_or(relative)
                    .trim_end_matches(".prefab.json");
                self.templates
                    .iter()
                    .find(|template| template.kind == DocKind::Prefab && template.id == stem)
                    .map(|template| template.doc.clone())
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("prefab not found {relative}"),
                        )
                    })?
            }
        };

        let parent = parent.filter(|index| *index < self.document().doc.entities.len());
        let inserted = self.edit("instantiate prefab", |document| {
            document.doc.instantiate_prefab(&prefab, parent, relative)
        });
        if let Some(root) = inserted.first() {
            self.selection = vec![*root];
            self.selection_active = Some(*root);
            if let Some(id) = self.document().doc.id_of_index(*root) {
                self.expanded.insert(id);
            }
        }
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("instantiated {relative} ({} entities)", inserted.len()),
        );
        Ok(inserted.len())
    }

    pub fn save_selection_as_prefab(&mut self, name: &str) -> io::Result<String> {
        self.save_selection_as_prefab_at(Some("assets/prefabs"), name)
    }

    pub fn save_selection_as_prefab_at(
        &mut self,
        folder: Option<&str>,
        name: &str,
    ) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot save prefab",
            )
        })?;
        let index = self
            .primary_selection()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no entity selected"))?;
        let doc = self.document().doc.extract_subtree(&[index]);
        let doc = self.pruned_doc(doc);
        let stem = project::sanitize_stem(name);
        let folder = folder
            .filter(|folder| folder.starts_with("assets/"))
            .unwrap_or("assets/prefabs");
        let mut path = format!("{folder}/{stem}.prefab.json");
        let mut suffix = 2;
        while root.join(&path).exists() {
            path = format!("{folder}/{stem}-{suffix}.prefab.json");
            suffix += 1;
        }
        project::write_document(&root, &path, &doc)?;
        self.refresh_project_files();
        self.refresh_assets();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("saved as prefab {path} ({} entities)", doc.entities.len()),
        );
        Ok(path)
    }

    pub fn apply_prefab_instance(&mut self, entity_id: u32) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot apply prefab changes",
            )
        })?;
        let index =
            self.document().doc.index_of_id(entity_id).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "instance does not exist")
            })?;
        let link = self
            .document()
            .doc
            .entities
            .get(index)
            .and_then(|entity| entity.prefab_link())
            .map(str::to_string)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a prefab instance"))?;
        if !link.starts_with("assets/") || !link.ends_with(".prefab.json") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("illegal prefab path {link}"),
            ));
        }

        let doc = self.document().doc.extract_subtree(&[index]);
        let doc = self.pruned_doc(doc);
        project::write_document(&root, &link, &doc)?;
        self.refresh_project_files();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!(
                "applied prefab changes {link} ({} entities)",
                doc.entities.len()
            ),
        );
        Ok(link)
    }

    pub fn open_prefab_from_instance(
        &mut self,
        scene_index: usize,
        entity_id: u32,
    ) -> io::Result<()> {
        let Some(scene) = self.docs.get(scene_index) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "document does not exist",
            ));
        };
        if matches!(scene.kind, DocKind::Prefab) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only open prefab from scene instance",
            ));
        }
        let Some(link) = scene
            .doc
            .entities
            .iter()
            .find(|entity| entity.id == Some(entity_id))
            .and_then(|entity| entity.prefab_link())
        else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "this entity is not a prefab instance",
            ));
        };
        let link = link.to_string();
        let scene_title = scene.title();

        self.open_project_file(&link)?;
        let Some(prefab_index) = self
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some(link.as_str()))
        else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "prefab document not open",
            ));
        };
        self.docs[prefab_index].prefab_origin = Some(PrefabOrigin {
            scene_index,
            entity_id,
            scene_title,
        });
        Ok(())
    }

    pub fn return_from_prefab(&mut self, prefab_index: usize) -> bool {
        let Some(origin) = self
            .docs
            .get(prefab_index)
            .and_then(|doc| doc.prefab_origin.clone())
        else {
            return false;
        };
        if origin.scene_index >= self.docs.len() {
            return false;
        }
        let clean = self
            .docs
            .get(prefab_index)
            .map(|doc| !doc.is_dirty())
            .unwrap_or(false);

        self.set_active(origin.scene_index);
        let mut target = origin.entity_id;
        if clean {
            match self.revert_prefab_instance(origin.entity_id) {
                Ok(id) => target = id,
                Err(error) => self.log(
                    LogSource::Editor,
                    LogLevel::Warn,
                    format!("sync instance failed when returning to scene: {error}"),
                ),
            }
        }
        if let Some(index) = self.document().doc.index_of_id(target) {
            self.select(index, false);
        }
        true
    }

    pub fn revert_prefab_instance(&mut self, entity_id: u32) -> io::Result<u32> {
        let prefab = self.prefab_document_for(entity_id)?;
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "instance does not exist",
            ));
        };
        let (name, parent_id, link) = {
            let entity = &self.document().doc.entities[index];
            (
                entity.display_name(),
                entity.parent,
                entity.prefab_link().unwrap_or_default().to_string(),
            )
        };

        let root = self.edit("restore prefab instance", |document| {
            document.doc.remove_entities(&[index]);
            let parent = parent_id.and_then(|id| document.doc.index_of_id(id));
            let inserted = document.doc.instantiate_prefab(&prefab, parent, &link);
            let root = inserted.first().copied();
            if let Some(root) = root {
                if let Some(entity) = document.doc.entities.get_mut(root) {
                    entity.name = Some(name.clone());
                }
            }
            root
        });
        let Some(root) = root else {
            return Err(io::Error::other("prefab generated no entities"));
        };
        let root_id = self
            .document()
            .doc
            .entities
            .get(root)
            .and_then(|entity| entity.id)
            .ok_or_else(|| io::Error::other("new instance has no entity id"))?;
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("restored prefab instance {name}"),
        );
        Ok(root_id)
    }

    pub fn sync_prefab_instances(&mut self, prefab_path: &str) -> io::Result<usize> {
        let ids: Vec<u32> = self
            .document()
            .doc
            .entities
            .iter()
            .filter(|entity| entity.prefab_link() == Some(prefab_path))
            .filter_map(|entity| entity.id)
            .collect();
        let mut synced = 0;
        for id in ids.into_iter().rev() {
            if self.revert_prefab_instance(id).is_ok() {
                synced += 1;
            }
        }
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("synced {synced} {prefab_path} instances"),
        );
        Ok(synced)
    }

    pub fn prefab_document_for(&self, entity_id: u32) -> io::Result<SceneDoc> {
        let path = self
            .document()
            .doc
            .entities
            .iter()
            .find(|entity| entity.id == Some(entity_id))
            .and_then(|entity| entity.prefab_link())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not a prefab instance"))?
            .to_string();

        if let Some(root) = &self.project_root {
            return project::read_document(root, &path);
        }

        let stem = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .trim_end_matches(".prefab.json");
        self.templates
            .iter()
            .find(|template| template.kind == DocKind::Prefab && template.id == stem)
            .map(|template| template.doc.clone())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("prefab not found {path} (fixture-only mode needs same-name template)"),
                )
            })
    }

    pub fn unlink_prefab(&mut self, index: usize) -> bool {
        let linked = self
            .document()
            .doc
            .entities
            .get(index)
            .map(|entity| entity.has_component(PREFAB_LINK_COMPONENT))
            .unwrap_or(false);
        if !linked {
            return false;
        }
        self.edit("unlink prefab", |document| {
            document.doc.remove_component(index, PREFAB_LINK_COMPONENT);
        });
        true
    }

    pub fn prefab_links(&self) -> Vec<(usize, String)> {
        self.document().doc.prefab_links()
    }

    pub fn prefab_component_name() -> &'static str {
        PREFAB_LINK_COMPONENT
    }

    pub fn prefab_overrides(&self, root_index: usize) -> io::Result<Vec<PrefabOverride>> {
        let doc = &self.document().doc;
        let root = doc
            .entities
            .get(root_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entity does not exist"))?;
        let root_id = root
            .id
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "instance root has no id"))?;
        let prefab = self.prefab_document_for(root_id)?;
        let Some(prefab_root_id) = prefab.entities.first().and_then(|entity| entity.id) else {
            return Ok(Vec::new());
        };
        let base = root_id as i64 - prefab_root_id as i64;

        let mut out = Vec::new();
        for (index, source) in prefab.entities.iter().enumerate() {
            let Some(source_id) = source.id else { continue };
            let instance_id = (source_id as i64 + base).try_into().unwrap_or(u32::MAX);
            let Some(instance_index) = doc.index_of_id(instance_id) else {
                continue;
            };
            let instance = &doc.entities[instance_index];
            out.extend(overrides_between(instance, instance_index, source, index));
        }
        Ok(out)
    }

    pub fn revert_prefab_override(&mut self, target: &PrefabOverride) -> bool {
        let Some(id) = self.document().doc.id_of_index(target.entity) else {
            return false;
        };
        let mut changed = false;
        self.edit("rollback prefab rewrite", |document| {
            let Some(index) = document.doc.index_of_id(id) else {
                return;
            };
            let entity = &mut document.doc.entities[index];
            if target.component == "transform" {
                let value = &target.prefab_value;
                match target.field.as_str() {
                    "position" | "scale" => {
                        if let Some(array) = json_vec3(value) {
                            if target.field == "position" {
                                entity.transform.position = array;
                            } else {
                                entity.transform.scale = array;
                            }
                            changed = true;
                        }
                    }
                    "rotation" => {
                        if let Some(array) = json_quat(value) {
                            entity.transform.rotation = array;
                            changed = true;
                        }
                    }
                    _ => {}
                }
            } else if let Some(component) = entity.components.get_mut(&target.component) {
                set_json_field(component, &target.field, target.prefab_value.clone());
                changed = true;
            }
        });
        changed
    }

    pub fn apply_prefab_override(
        &mut self,
        root_index: usize,
        target: &PrefabOverride,
    ) -> io::Result<()> {
        let root_id = self
            .document()
            .doc
            .entities
            .get(root_index)
            .and_then(|entity| entity.id)
            .and_then(|id| {
                self.document()
                    .doc
                    .entities
                    .iter()
                    .find(|e| e.id == Some(id))
            })
            .and_then(|entity| entity.prefab_link())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not a prefab instance"))?
            .to_string();

        let prefab = self.prefab_document_for(root_id_of(self, root_index)?)?;
        let Some(prefab_root_id) = prefab.entities.first().and_then(|entity| entity.id) else {
            return Ok(());
        };
        let base = root_id_of(self, root_index)? as i64 - prefab_root_id as i64;
        let instance_id = self
            .document()
            .doc
            .id_of_index(target.entity)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entity has no id"))?;
        let prefab_id = (instance_id as i64 - base) as u32;

        let mut updated = prefab.clone();
        let index = updated.index_of_id(prefab_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "corresponding entity not found in prefab",
            )
        })?;
        let entity = &mut updated.entities[index];
        if target.component == "transform" {
            match target.field.as_str() {
                "position" => {
                    if let Some(array) = json_vec3(&target.instance_value) {
                        entity.transform.position = array;
                    }
                }
                "scale" => {
                    if let Some(array) = json_vec3(&target.instance_value) {
                        entity.transform.scale = array;
                    }
                }
                "rotation" => {
                    if let Some(array) = json_quat(&target.instance_value) {
                        entity.transform.rotation = array;
                    }
                }
                _ => {}
            }
        } else if let Some(component) = entity.components.get_mut(&target.component) {
            set_json_field(component, &target.field, target.instance_value.clone());
        }

        let open_clean = self
            .docs
            .iter()
            .find(|document| document.path.as_deref() == Some(root_id.as_str()))
            .map(|document| !document.is_dirty());
        if let Some(document) = self
            .docs
            .iter_mut()
            .find(|document| document.path.as_deref() == Some(root_id.as_str()))
        {
            document.doc = updated.clone();
            document.dirty = true;
        }
        if open_clean == Some(false) {
            return Ok(());
        }
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no project root"))?;
        let disk = self.pruned_doc(updated);
        project::write_document(&root, &root_id, &disk)?;
        Ok(())
    }

    pub fn unpack_prefab(&mut self, root_index: usize) -> usize {
        let doc = &self.document().doc;
        let mut subtree = doc.descendants(root_index);
        subtree.push(root_index);
        let linked: Vec<usize> = subtree
            .into_iter()
            .filter(|index| {
                doc.entities
                    .get(*index)
                    .map(|entity| entity.has_component(PREFAB_LINK_COMPONENT))
                    .unwrap_or(false)
            })
            .collect();
        if linked.is_empty() {
            return 0;
        }
        self.edit("unpack prefab", |document| {
            for index in &linked {
                document.doc.remove_component(*index, PREFAB_LINK_COMPONENT);
            }
        });
        linked.len()
    }
}
