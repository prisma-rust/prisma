//! Project files, assets, sidecar metadata rewriting, recovery, and scene manifests.

use super::*;

pub(super) fn state_machine_runtime_paths(source: &str) -> Vec<String> {
    let source = source.trim();
    if source.is_empty() {
        return Vec::new();
    }
    let mut paths = vec![source.to_string()];
    if let Some(relative) = source.strip_prefix("assets/") {
        paths.push(relative.to_string());
    }
    if let Some(stem) = source.strip_suffix(".statemachine.json") {
        let stem = stem.strip_prefix("assets/").unwrap_or(stem);
        paths.push(format!("{stem}.rasm"));
    }
    paths.sort();
    paths.dedup();
    paths
}

pub(super) fn rewrite_animator_state(
    doc: &mut SceneDoc,
    machine_paths: &[String],
    old: &str,
    new: &str,
) -> usize {
    let mut rewritten = 0;
    for entity in &mut doc.entities {
        let Some(animator) = entity
            .components
            .get_mut("prism_engine::scene::Animator")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let machine = animator
            .get("state_machine_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if !machine_paths.iter().any(|path| path == machine) {
            continue;
        }
        if animator.get("state").and_then(Value::as_str) == Some(old) {
            animator.insert("state".to_string(), Value::String(new.to_string()));
            rewritten += 1;
        }
    }
    rewritten
}

pub(super) fn state_machine_source_path(machine: &str) -> Option<String> {
    let machine = machine.trim();
    if machine.is_empty() {
        return None;
    }
    let source = if let Some(stem) = machine.strip_suffix(".rasm") {
        format!("{stem}.statemachine.json")
    } else if machine.ends_with(".statemachine.json") {
        machine.to_string()
    } else {
        return None;
    };

    if source.starts_with("assets/") {
        Some(source)
    } else {
        Some(format!("assets/{source}"))
    }
}

pub(super) fn document_dir(path: Option<&str>) -> String {
    path.and_then(|path| path.rsplit_once('/'))
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_else(|| "assets".to_string())
}

pub(super) fn remove_asset_sidecar(path: &Path) {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".rmeta");
    let meta = path.with_file_name(name);
    if let Err(error) = std::fs::remove_file(&meta) {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!("asset meta: failed to remove {}: {error}", meta.display());
        }
    }
}

/// GUIDs of the assets moved by a rename (file or directory subtree).
pub(super) fn moved_guids(root: &Path, moves: &[(String, String)]) -> BTreeSet<String> {
    let assets = project::scan_assets(root);
    let mut out = BTreeSet::new();
    for (from, to) in moves {
        let from_prefix = format!("{from}/");
        let to_prefix = format!("{to}/");
        for asset in &assets {
            // `rewrite_moved_references` runs after the filesystem move, so the
            // assets are already under `to`; match both sides for robustness.
            if asset.path == *from
                || asset.path == *to
                || asset.path.starts_with(&from_prefix)
                || asset.path.starts_with(&to_prefix)
            {
                if let Some(guid) = &asset.guid {
                    out.insert(guid.to_ascii_lowercase());
                }
            }
        }
    }
    out
}

/// Does the path slot behind `field` have a sibling GUID that still exists?
pub(super) fn reference_has_live_guid(
    doc: &SceneDoc,
    entity: usize,
    field: &str,
    live_guids: &BTreeSet<String>,
) -> bool {
    let Some((component, path_field)) = field.split_once('.') else {
        return false;
    };
    let Some(payload) = doc
        .entities
        .get(entity)
        .and_then(|entity| entity.components.get(component))
        .and_then(Value::as_object)
    else {
        return false;
    };
    let leaf = path_field.split(['.', '[']).next().unwrap_or(path_field);
    let Some(guid_field) = crate::assets::guid_field_for_path(component, leaf) else {
        return false;
    };
    let Some(value) = payload.get(&guid_field) else {
        return false;
    };
    let index = path_field
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .and_then(|(number, _)| number.parse::<usize>().ok());
    let guid = match (index, value) {
        (Some(index), Value::Array(items)) => items.get(index).and_then(Value::as_str),
        (None, Value::String(guid)) => Some(guid.as_str()),
        _ => None,
    };
    guid.is_some_and(|guid| live_guids.contains(&guid.to_ascii_lowercase()))
}

impl Workspace {
    pub fn load_project(&mut self, root: PathBuf) -> io::Result<usize> {
        let files = project::scan_documents(&root);
        let mut opened: Vec<Document> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for file in &files {
            match project::read_document(&root, &file.path) {
                Ok(doc) => {
                    let mut document = Document::new(file.kind, Some(file.path.clone()), doc);
                    document.note_disk_state(Some(&root));
                    opened.push(document);
                }
                Err(error) => {
                    failed.push(file.path.clone());
                    self.log(
                        LogSource::Editor,
                        LogLevel::Error,
                        format!("read {} failed: {error}", file.path),
                    );
                }
            }
        }
        let has_project_files = !files.is_empty();
        if has_project_files && opened.is_empty() {
            self.log(
                LogSource::Editor,
                LogLevel::Error,
                format!(
                    "all {} documents in project failed to load; kept empty project scene, not showing editor example documents",
                    failed.len()
                ),
            );
        }
        self.project_load_failures = failed;

        self.project_root = Some(root);
        self.project_files = files;
        if let Some(root) = self.project_root.clone() {
            let generated = project::ensure_asset_metas(&root);
            if generated > 0 {
                self.log(
                    LogSource::Editor,
                    LogLevel::Info,
                    format!("generated .rmeta for {generated} assets"),
                );
            }
            self.assets = project::scan_assets(&root);
            self.tree = AssetTree::build(&self.assets);
            for folder in project::scan_asset_dirs(&root) {
                self.tree.insert_dir(&folder);
            }
        }
        match SceneManifest::read(
            self.project_root
                .as_deref()
                .expect("project_root just assigned"),
        ) {
            Ok(manifest) => self.scene_manifest = manifest,
            Err(error) => self.log(
                LogSource::Editor,
                LogLevel::Warn,
                format!("read assets/scenes.json failed: {error}"),
            ),
        }
        if !opened.is_empty() {
            self.docs = opened;

            self.active = self
                .docs
                .iter()
                .position(|document| document.kind == DocKind::Scene)
                .unwrap_or(0);
            self.clear_selection();
            self.expanded.clear();
        } else if has_project_files {
            // Every project document failed to parse. Keep an empty scene instead
            // of the sample fixture: otherwise the hierarchy and the Game view
            // show the editor's demo scene while the status bar says the project,
            // which reads as "my project looks like this" (#1020).
            self.docs = vec![Document::new(DocKind::Scene, None, SceneDoc::default())];
            self.active = 0;
            self.clear_selection();
            self.expanded.clear();
        }
        self.hydrate_documents();
        let count = self.docs.len();
        self.log(
            LogSource::System,
            LogLevel::Info,
            format!(
                "project: {} documents / {} files",
                count,
                self.project_files.len()
            ),
        );
        Ok(count)
    }

    pub fn delete_asset(&mut self, path: &str) -> io::Result<()> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot delete asset",
            )
        })?;
        if !path.starts_with("assets/") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only files within assets/ may be deleted",
            ));
        }
        if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "scene / prefab please use document panel",
            ));
        }
        let full = root.join(path);
        if !full.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} does not exist"),
            ));
        }
        std::fs::remove_file(&full)?;
        remove_asset_sidecar(&full);
        self.refresh_project_files();
        self.log(LogSource::Editor, LogLevel::Info, format!("deleted {path}"));
        Ok(())
    }

    pub fn delete_document(&mut self, path: &str) -> io::Result<()> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot delete document",
            )
        })?;
        if !path.starts_with("assets/")
            || !(path.ends_with(".scene.json") || path.ends_with(".prefab.json"))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only .scene.json / .prefab.json under assets/ may be deleted",
            ));
        }
        let full = root.join(path);
        if !full.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} does not exist"),
            ));
        }
        std::fs::remove_file(&full)?;
        remove_asset_sidecar(&full);

        let removed = self
            .docs
            .iter()
            .position(|document| document.path.as_deref() == Some(path));
        self.docs
            .retain(|document| document.path.as_deref() != Some(path));
        if let Some(index) = removed {
            if index < self.active {
                self.active -= 1;
            }
        }
        self.active = self.active.min(self.docs.len().saturating_sub(1));
        self.clear_selection();
        self.expanded.clear();
        if self.docs.is_empty() {
            self.new_document(DocKind::Scene, None);
        }

        let doc_relative = path.trim_start_matches("assets/");
        if let Some(position) = self.scene_manifest.scenes.iter().position(|entry| {
            crate::manifest::doc_relative_path(&entry.path).as_deref() == Some(doc_relative)
        }) {
            self.scene_manifest.remove(position);
            self.persist_scene_manifest();
        }
        self.project_files.retain(|file| file.path != path);
        self.log(LogSource::Editor, LogLevel::Info, format!("deleted {path}"));
        Ok(())
    }

    pub fn rename_asset(&mut self, from: &str, to: &str) -> io::Result<(usize, Vec<String>)> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot rename asset",
            )
        })?;
        if from.ends_with(".scene.json") || from.ends_with(".prefab.json") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "scene / prefab please use Save As or document panel",
            ));
        }
        project::rename_path(&root, from, to)?;

        let (references, affected) =
            self.rewrite_moved_references(&root, &[(from.to_string(), to.to_string())]);
        Ok((references, affected))
    }

    pub(super) fn rewrite_moved_references(
        &mut self,
        root: &Path,
        moves: &[(String, String)],
    ) -> (usize, Vec<String>) {
        let protected_guids = moved_guids(root, moves);
        let mut references = 0;
        let mut affected = Vec::new();
        let mut ambiguous: Vec<String> = Vec::new();
        let mut open_paths: Vec<String> = Vec::new();

        for document in &mut self.docs {
            let doc_dir = document_dir(document.path.as_deref());
            for (from, to) in moves {
                let rewrite = document.doc.rewrite_asset_references_with_guids(
                    &doc_dir,
                    from,
                    to,
                    &protected_guids,
                );
                ambiguous.extend(rewrite.ambiguous);
                if rewrite.rewritten > 0 {
                    document.dirty = true;
                    document.mark_dirty();
                    references += rewrite.rewritten;
                    affected.push(document.title());
                }
                if document.path.as_deref() == Some(from.as_str()) {
                    document.path = Some(to.clone());
                }
            }
            if let Some(path) = document.path.clone() {
                open_paths.push(path);
            }
        }

        let closed: Vec<String> = self
            .project_files
            .iter()
            .filter(|file| matches!(file.kind, DocKind::Scene | DocKind::Prefab))
            .map(|file| file.path.clone())
            .filter(|path| !open_paths.contains(path))
            .collect();
        for path in closed {
            let mut doc = match project::read_document(root, &path) {
                Ok(doc) => doc,
                Err(error) => {
                    log::warn!("rename: cannot read {path}: {error}");
                    continue;
                }
            };
            let doc_dir = document_dir(Some(&path));
            let mut rewritten = 0;
            for (from, to) in moves {
                let rewrite =
                    doc.rewrite_asset_references_with_guids(&doc_dir, from, to, &protected_guids);
                ambiguous.extend(rewrite.ambiguous);
                rewritten += rewrite.rewritten;
            }
            if rewritten == 0 {
                continue;
            }
            let doc = self.pruned_doc(doc);
            match project::write_document(root, &path, &doc) {
                Ok(()) => {
                    references += rewritten;
                    affected.push(path);
                }
                Err(error) => log::warn!("rename: cannot write {path}: {error}"),
            }
        }

        affected.sort();
        affected.dedup();

        self.refresh_project_files();
        self.refresh_assets();
        if !ambiguous.is_empty() {
            ambiguous.sort();
            ambiguous.dedup();
            self.log(
                LogSource::Editor,
                LogLevel::Warn,
                format!(
                    "reference at {} cannot be auto-rewritten (not `assets/` or relative path): {}",
                    ambiguous.len(),
                    ambiguous.join(", ")
                ),
            );
        }
        (references, affected)
    }

    pub fn document_dir(&self) -> String {
        document_dir(self.document().path.as_deref())
    }

    pub fn missing_references(&self, index: usize) -> Vec<(String, String)> {
        self.broken_references()
            .into_iter()
            .filter(|(entity, _, _)| *entity == index)
            .map(|(_, field, value)| (field, value))
            .collect()
    }

    /// Every broken asset reference in the active document:
    /// `(entity, field, value)`.
    /// A slot whose sibling `*_guid` still resolves to an existing asset is
    /// **not** broken even if its stored path is stale (#466); path resolution
    /// remains the fallback for legacy references.
    pub fn broken_references(&self) -> Vec<(usize, String, String)> {
        let doc = &self.document().doc;
        let dir = self.document_dir();
        let paths = self.asset_paths();
        let live_guids: BTreeSet<String> = self
            .assets
            .iter()
            .filter_map(|asset| asset.guid.as_deref())
            .map(str::to_ascii_lowercase)
            .collect();
        doc.reference_values()
            .into_iter()
            .filter(|(entity, field, value)| {
                if reference_has_live_guid(doc, *entity, field, &live_guids) {
                    return false;
                }
                crate::assets::resolve_reference(&paths, &dir, value).is_none()
            })
            .collect()
    }

    pub fn asset_paths(&self) -> Vec<String> {
        self.assets.iter().map(|asset| asset.path.clone()).collect()
    }

    pub fn refresh_project_files(&mut self) {
        if let Some(root) = &self.project_root {
            self.project_files = project::scan_documents(root);
        }
    }

    pub fn read_asset_json(&self, path: &str) -> io::Result<Value> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no project root bound"))?;
        project::read_json_asset(&root, path)
    }

    pub fn read_asset_bytes(&self, path: &str) -> io::Result<Vec<u8>> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no project root bound"))?;
        std::fs::read(root.join(path))
    }

    pub fn read_anim_graph_layout(&self, graph_path: &str) -> AnimGraphLayout {
        let Some(root) = self.project_root.as_deref() else {
            return AnimGraphLayout::default();
        };
        let path = AnimGraphLayout::path_for(graph_path);
        let Ok(text) = std::fs::read_to_string(root.join(path)) else {
            return AnimGraphLayout::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return AnimGraphLayout::default();
        };
        AnimGraphLayout::from_json(&value).unwrap_or_default()
    }

    pub fn write_anim_graph_layout(
        &mut self,
        graph_path: &str,
        layout: &AnimGraphLayout,
    ) -> io::Result<()> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no project root bound"))?;
        project::write_json_asset(
            &root,
            &AnimGraphLayout::path_for(graph_path),
            &layout.to_json(),
        )
    }

    pub fn write_asset_json(&mut self, path: &str, value: &Value) -> io::Result<()> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no project root bound"))?;
        project::write_json_asset(&root, path, value)?;
        self.refresh_assets();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("saved material {path}"),
        );
        Ok(())
    }

    pub fn record_asset_edit(&mut self, path: &str, before: &Value, after: &Value) {
        self.asset_edits.record(path, before, after);
    }

    pub fn can_undo_asset_edit(&self) -> bool {
        self.asset_edits.can_undo()
    }

    pub fn can_redo_asset_edit(&self) -> bool {
        self.asset_edits.can_redo()
    }

    pub fn record_asset_edit_group(&mut self, edits: Vec<crate::asset_edit::AssetEdit>) {
        self.asset_edits.record_group(edits);
    }

    pub fn undo_asset_edit(&mut self) -> Result<Option<Vec<String>>, String> {
        let Some(edits) = self.asset_edits.undo() else {
            return Ok(None);
        };
        let mut written = Vec::with_capacity(edits.len());
        for edit in edits {
            self.write_asset_json(&edit.path, &edit.before)
                .map_err(|error| format!("write {} failed: {error}", edit.path))?;
            written.push(edit.path);
        }
        Ok(Some(written))
    }

    pub fn rename_state_machine_state(
        &mut self,
        machine_path: &str,
        index: usize,
        new_name: &str,
    ) -> Result<Vec<String>, String> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("a state needs a non-empty name".to_string());
        }
        let before = self
            .read_asset_json(machine_path)
            .map_err(|error| format!("read {machine_path} failed: {error}"))?;
        let view = crate::anim_graph_view::parse_state_machine_view(&before)?;
        let old_name = view
            .states
            .get(index)
            .ok_or_else(|| format!("state machine has no state #{index}"))?
            .name
            .clone();
        if old_name == new_name {
            return Ok(Vec::new());
        }
        let mut machine = before.clone();
        crate::anim_graph_view::rename_state(&mut machine, index, new_name)?;
        let updated = self.rewrite_animator_state_references(machine_path, &old_name, new_name)?;
        self.write_asset_json(machine_path, &machine)
            .map_err(|error| format!("write {machine_path} failed: {error}"))?;
        self.record_asset_edit(machine_path, &before, &machine);
        Ok(updated)
    }

    pub(super) fn rewrite_animator_state_references(
        &mut self,
        machine_path: &str,
        old: &str,
        new: &str,
    ) -> Result<Vec<String>, String> {
        let Some(root) = self.project_root.clone() else {
            return Ok(Vec::new());
        };
        let paths = state_machine_runtime_paths(machine_path);
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut affected = Vec::new();
        let mut open_paths = Vec::new();
        for document in &mut self.docs {
            if rewrite_animator_state(&mut document.doc, &paths, old, new) > 0 {
                document.dirty = true;
                document.mark_dirty();
                affected.push(document.path.clone().unwrap_or_else(|| document.title()));
            }
            if let Some(path) = document.path.clone() {
                open_paths.push(path);
            }
        }
        let closed: Vec<String> = self
            .project_files
            .iter()
            .filter(|file| matches!(file.kind, DocKind::Scene | DocKind::Prefab))
            .map(|file| file.path.clone())
            .filter(|path| !open_paths.contains(path))
            .collect();
        for path in closed {
            let mut doc = match project::read_document(&root, &path) {
                Ok(doc) => doc,
                Err(error) => {
                    log::warn!("state rename: cannot read {path}: {error}");
                    continue;
                }
            };
            if rewrite_animator_state(&mut doc, &paths, old, new) == 0 {
                continue;
            }
            let doc = self.pruned_doc(doc);
            match project::write_document(&root, &path, &doc) {
                Ok(()) => affected.push(path),
                Err(error) => log::warn!("state rename: cannot write {path}: {error}"),
            }
        }
        affected.sort();
        affected.dedup();
        Ok(affected)
    }

    pub fn redo_asset_edit(&mut self) -> Result<Option<Vec<String>>, String> {
        let Some(edits) = self.asset_edits.redo() else {
            return Ok(None);
        };
        let mut written = Vec::with_capacity(edits.len());
        for edit in edits {
            self.write_asset_json(&edit.path, &edit.after)
                .map_err(|error| format!("write {} failed: {error}", edit.path))?;
            written.push(edit.path);
        }
        Ok(Some(written))
    }

    pub fn rename_anim_graph_parameter(
        &mut self,
        graph_path: &str,
        old: &str,
        new: &str,
    ) -> Result<Vec<String>, String> {
        let mut graph = self
            .read_asset_json(graph_path)
            .map_err(|error| format!("read {graph_path} failed: {error}"))?;
        crate::anim_graph_view::rename_parameter(&mut graph, old, new)?;
        let mut machines: Vec<String> = Vec::new();
        for node in graph
            .get("nodes")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let path = node
                .get("state_machine")
                .or_else(|| node.get("machine"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            let Some(source) = state_machine_source_path(path) else {
                continue;
            };
            if !machines.contains(&source) {
                machines.push(source);
            }
        }

        let mut updates = Vec::new();
        for source in machines {
            let Ok(mut document) = self.read_asset_json(&source) else {
                continue;
            };
            if crate::anim_graph_view::rename_condition_parameter(&mut document, old, new)? {
                updates.push((source, document));
            }
        }

        let graph_before = self
            .read_asset_json(graph_path)
            .map_err(|error| format!("read {graph_path} failed: {error}"))?;
        let mut group = Vec::new();
        let mut written = Vec::new();
        for (path, document) in updates {
            if let Ok(before) = self.read_asset_json(&path) {
                group.push(crate::asset_edit::AssetEdit {
                    path: path.clone(),
                    before,
                    after: document.clone(),
                });
            }
            self.write_asset_json(&path, &document)
                .map_err(|error| format!("write {path} failed: {error}"))?;
            written.push(path);
        }
        group.push(crate::asset_edit::AssetEdit {
            path: graph_path.to_string(),
            before: graph_before,
            after: graph.clone(),
        });
        self.write_asset_json(graph_path, &graph)
            .map_err(|error| format!("write {graph_path} failed: {error}"))?;
        self.record_asset_edit_group(group);
        Ok(written)
    }

    pub fn refresh_assets(&mut self) {
        let Some(root) = self.project_root.clone() else {
            return;
        };

        let generated = project::ensure_asset_metas(&root);
        if generated > 0 {
            self.log(
                LogSource::Editor,
                LogLevel::Info,
                format!("generated .rmeta for {generated} assets"),
            );
        }
        self.assets = project::scan_assets(&root);
        let tree = {
            let mut tree = AssetTree::build(&self.assets);
            for folder in project::scan_asset_dirs(&root) {
                tree.insert_dir(&folder);
            }
            tree
        };
        self.expanded_assets
            .retain(|path| tree.find(path).is_some());
        self.tree = tree;
    }

    pub fn create_asset_folder(&mut self, relative: &str) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot create directory",
            )
        })?;
        let relative = relative.trim_end_matches('/');
        if let Some(problem) = project::asset_path_problem(relative) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("directory path invalid: {problem}"),
            ));
        }
        if !relative.starts_with("assets/") || relative == "assets" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory must be within assets/",
            ));
        }
        if root.join(relative).exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{relative} already exists"),
            ));
        }
        project::create_folder(&root, relative)?;
        if let Some(parent) = relative.rsplit_once('/').map(|(parent, _)| parent) {
            self.expanded_assets.insert(parent.to_string());
        }
        self.refresh_assets();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("created directory {relative}"),
        );
        Ok(relative.to_string())
    }

    pub fn rename_folder(&mut self, from: &str, to: &str) -> io::Result<(usize, Vec<String>)> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot rename directory",
            )
        })?;
        let from = from.trim_end_matches('/');
        let to = to.trim_end_matches('/');
        if let Some(problem) = project::asset_path_problem(to) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("directory path invalid: {problem}"),
            ));
        }
        if !from.starts_with("assets/") || !to.starts_with("assets/") || to == "assets" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory must be within assets/",
            ));
        }
        if !root.join(from).is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{from} is not a directory"),
            ));
        }
        if to == from || to.starts_with(&format!("{from}/")) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target cannot be directory itself or its subdirectory",
            ));
        }

        let prefix = format!("{from}/");
        let mut moves: Vec<(String, String)> = self
            .assets
            .iter()
            .filter(|asset| asset.path.starts_with(&prefix))
            .map(|asset| {
                (
                    asset.path.clone(),
                    format!("{to}/{}", &asset.path[prefix.len()..]),
                )
            })
            .collect();

        for file in &self.project_files {
            if file.path.starts_with(&prefix) && !moves.iter().any(|(from, _)| from == &file.path) {
                moves.push((
                    file.path.clone(),
                    format!("{to}/{}", &file.path[prefix.len()..]),
                ));
            }
        }

        project::rename_path(&root, from, to)?;

        let (references, affected) = self.rewrite_moved_references(&root, &moves);

        let expansions: Vec<String> = self.expanded_assets.iter().cloned().collect();
        for path in expansions {
            let next = if path == from {
                to.to_string()
            } else if let Some(rest) = path.strip_prefix(&prefix) {
                format!("{to}/{rest}")
            } else {
                continue;
            };
            self.expanded_assets.remove(&path);
            self.expanded_assets.insert(next);
        }

        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!(
                "renamed directory {from} -> {to} (rewrote {references} references, {} documents)",
                affected.len()
            ),
        );
        Ok((references, affected))
    }

    pub fn delete_folder(&mut self, path: &str) -> io::Result<()> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot delete directory",
            )
        })?;
        let path = path.trim_end_matches('/');
        if !path.starts_with("assets/") || path == "assets" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only subdirectories within assets/ may be deleted",
            ));
        }
        let full = root.join(path);
        if !full.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} is not a directory"),
            ));
        }
        let count = std::fs::read_dir(&full)?.count();
        if count > 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("directory non-empty ({count} items), empty it first"),
            ));
        }
        std::fs::remove_dir(&full)?;
        self.expanded_assets.remove(path);
        self.refresh_assets();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("deleted directory {path}"),
        );
        Ok(())
    }

    pub fn duplicate_asset(&mut self, path: &str) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot copy asset",
            )
        })?;
        let target = crate::assets::unique_asset_path(&self.assets, path);
        project::copy_file(&root, path, &target)?;
        self.refresh_project_files();
        self.refresh_assets();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("copied {path} -> {target}"),
        );
        Ok(target)
    }

    pub fn move_asset_to_folder(&mut self, from: &str, folder: Option<&str>) -> io::Result<String> {
        let name = from.rsplit('/').next().unwrap_or(from);
        let folder = folder.unwrap_or("assets").trim_end_matches('/');
        let to = format!("{folder}/{name}");
        if to == from {
            return Ok(to);
        }
        if from.ends_with(".scene.json") || from.ends_with(".prefab.json") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "scene / prefab please use Save As to move",
            ));
        }
        self.rename_asset(from, &to)?;
        Ok(to)
    }

    pub fn asset_details(&self, path: &str) -> Option<AssetDetails> {
        let asset = self.assets.iter().find(|asset| asset.path == path)?.clone();
        let root = self.project_root.as_deref();
        let preview = crate::assets::asset_preview(root, &asset, 32 * 1024);

        let target_guids: BTreeSet<String> = asset
            .guid
            .iter()
            .map(|guid| guid.to_ascii_lowercase())
            .collect();
        let mut references = Vec::new();
        let collect = |document: &str, doc: &SceneDoc, out: &mut Vec<AssetReference>| {
            for (entity, field, _) in doc.field_references_with_guids(&asset.path, &target_guids) {
                out.push(AssetReference {
                    document: document.to_string(),
                    entity: doc
                        .entities
                        .get(entity)
                        .and_then(|entity| entity.id)
                        .unwrap_or(0),
                    entity_name: doc
                        .entities
                        .get(entity)
                        .map(|entity| entity.display_name())
                        .unwrap_or_else(|| "(deleted)".to_string()),
                    field,
                });
            }
        };
        for document in &self.docs {
            let name = document.path.clone().unwrap_or_else(|| document.title());
            collect(&name, &document.doc, &mut references);
        }
        if let Some(root) = root {
            for file in &self.project_files {
                if self
                    .docs
                    .iter()
                    .any(|document| document.path.as_deref() == Some(file.path.as_str()))
                {
                    continue;
                }
                if let Ok(doc) = project::read_document(root, &file.path) {
                    collect(&file.path, &doc, &mut references);
                }
            }
        }
        references.sort_by(|a, b| {
            (&a.document, a.entity, &a.field).cmp(&(&b.document, b.entity, &b.field))
        });
        references.dedup();

        Some(AssetDetails {
            guid: asset
                .guid
                .clone()
                .unwrap_or_else(|| crate::assets::asset_guid(path)),
            asset,
            preview,
            references,
        })
    }

    /// Lower-case GUIDs of an asset, for GUID-aware reference lookups.
    pub fn asset_guids(&self, path: &str) -> BTreeSet<String> {
        self.assets
            .iter()
            .filter(|asset| asset.path == path)
            .filter_map(|asset| asset.guid.as_deref())
            .map(str::to_ascii_lowercase)
            .collect()
    }

    pub fn open_project_file(&mut self, relative: &str) -> io::Result<()> {
        if let Some(index) = self
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some(relative))
        {
            self.set_active(index);
            return Ok(());
        }
        let Some(root) = self.project_root.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound (fixture-only mode)",
            ));
        };
        let doc = project::read_document(&root, relative)?;
        let kind = if relative.ends_with(".prefab.json") {
            DocKind::Prefab
        } else {
            DocKind::Scene
        };
        let mut document = Document::new(kind, Some(relative.to_string()), doc);
        document.note_disk_state(Some(&root));
        self.docs.push(document);
        self.active = self.docs.len() - 1;
        self.clear_selection();
        self.expanded.clear();
        self.hydrate_documents();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("opened {relative}"),
        );
        Ok(())
    }

    pub fn save_active(&mut self) -> io::Result<String> {
        let index = self.active;
        self.save_document(index)
    }

    pub fn save_conflict(&self, index: usize) -> Option<String> {
        let root = self.project_root.as_deref()?;
        let document = self.docs.get(index)?;
        let path = document.path.as_deref()?;
        if document.externally_modified {
            return Some(path.to_string());
        }
        let saved = document.saved_stamp?;
        match project::file_content_stamp(root, path) {
            Some(current) if current == saved => None,
            _ => Some(path.to_string()),
        }
    }

    pub fn check_external_changes(&mut self) -> usize {
        let Some(root) = self.project_root.clone() else {
            return 0;
        };
        let mut changed = 0;
        for document in &mut self.docs {
            if document.externally_modified {
                continue;
            }
            let Some(path) = document.path.clone() else {
                continue;
            };
            let differs = match (
                document.saved_stamp,
                project::file_content_stamp(&root, &path),
            ) {
                (None, None) => false,
                (Some(saved), Some(current)) => saved != current,
                _ => true,
            };
            if differs {
                document.externally_modified = true;
                changed += 1;
            }
        }
        if changed > 0 {
            self.log(
                LogSource::Editor,
                LogLevel::Warn,
                format!("{changed} documents modified on disk (prompt before save)"),
            );
        }
        changed
    }

    pub fn reload_document_from_disk(&mut self, index: usize) -> io::Result<()> {
        let root = self
            .project_root
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no project root bound"))?;
        let path = self
            .docs
            .get(index)
            .and_then(|document| document.path.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "document has no path yet"))?;
        let doc = project::read_document(&root, &path)?;
        {
            let document = self.docs.get_mut(index).expect("checked above");
            document.doc = doc;
            document.undo = crate::undo::UndoStack::new();
            document.dirty = false;
            document.mark_dirty();
            document.note_disk_state(Some(&root));
        }
        self.hydrate_documents();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("reloaded {path} from disk content"),
        );
        Ok(())
    }

    pub fn save_document(&mut self, index: usize) -> io::Result<String> {
        if let Some(path) = self.save_conflict(index) {
            self.pending_save_conflict = Some(index);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("{path} modified on disk"),
            ));
        }
        self.write_document_now(index)
    }

    pub fn save_document_forced(&mut self, index: usize) -> io::Result<String> {
        self.write_document_now(index)
    }

    pub(super) fn pruned_doc(&self, mut doc: SceneDoc) -> SceneDoc {
        doc.prune_defaults(&self.schema);
        doc
    }

    pub(super) fn write_document_now(&mut self, index: usize) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot save",
            )
        })?;

        let (path, doc) = {
            let document = self.docs.get(index).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "document does not exist")
            })?;
            let path = match &document.path {
                Some(path) => path.clone(),
                None => project::default_save_path(&root, document.kind, &document.title()),
            };
            (path, document.doc.clone())
        };
        let doc = self.pruned_doc(doc);
        project::write_document(&root, &path, &doc)?;
        {
            let document = self.docs.get_mut(index).expect("checked above");
            document.path = Some(path.clone());
            document.mark_saved();
            document.note_disk_state(Some(&root));
        }
        self.refresh_project_files();
        self.log(LogSource::Editor, LogLevel::Info, format!("saved {path}"));
        Ok(path)
    }

    pub fn save_active_if_dirty(&mut self) -> Option<io::Result<String>> {
        if !self.document().is_dirty() {
            return None;
        }
        Some(self.save_active())
    }

    pub fn write_recovery(&self) -> io::Result<usize> {
        let Some(root) = self.project_root.as_deref() else {
            return Ok(0);
        };
        let entries: Vec<crate::recovery::RecoveredDocument> = self
            .docs
            .iter()
            .filter(|document| document.is_dirty())
            .map(|document| crate::recovery::RecoveredDocument {
                path: document.path.clone(),
                kind: document.kind,
                document: document.doc.clone(),
            })
            .collect();
        crate::recovery::write(root, &entries)
    }

    pub fn recovery_pending(&self) -> io::Result<usize> {
        let Some(root) = self.project_root.as_deref() else {
            return Ok(0);
        };
        crate::recovery::read(root).map(|entries| entries.len())
    }

    pub fn recover_unsaved(&mut self) -> io::Result<usize> {
        let Some(root) = self.project_root.clone() else {
            return Ok(0);
        };
        let entries = crate::recovery::read(&root)?;
        let count = entries.len();
        let mut first = None;
        for entry in entries {
            let existing = entry.path.as_deref().and_then(|path| {
                self.docs.iter().position(|document| {
                    document.kind == entry.kind && document.path.as_deref() == Some(path)
                })
            });
            let index = match existing {
                Some(index) => {
                    let document = &mut self.docs[index];
                    document.doc = entry.document;
                    document.undo = crate::undo::UndoStack::new();
                    document.dirty = true;
                    document.externally_modified = false;
                    document.mark_dirty();
                    index
                }
                None => {
                    let mut document = Document::new(entry.kind, entry.path, entry.document);
                    document.dirty = true;
                    self.docs.push(document);
                    self.docs.len() - 1
                }
            };
            first.get_or_insert(index);
        }
        if let Some(index) = first {
            self.set_active(index);
        }
        crate::recovery::clear(&root)?;
        if count > 0 {
            self.log(
                LogSource::Editor,
                LogLevel::Warn,
                format!("recovered {count} documents unsaved at last exit (still unsaved)"),
            );
        }
        Ok(count)
    }

    pub fn discard_recovery(&mut self) -> io::Result<()> {
        let Some(root) = self.project_root.as_deref() else {
            return Ok(());
        };
        crate::recovery::clear(root)?;
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            "discarded unsaved snapshot at exit",
        );
        Ok(())
    }

    pub fn save_active_as(&mut self, input: &str) -> io::Result<String> {
        let root = self.project_root.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound, cannot save",
            )
        })?;
        let kind = self.document().kind;
        let path = project::normalize_save_path(kind, input);
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path invalid (cannot be empty, contain .., or absolute path)",
            ));
        }
        if !path.starts_with("assets/") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only write within assets/",
            ));
        }
        let doc = self.document().doc.clone();
        let doc = self.pruned_doc(doc);
        project::write_document(&root, &path, &doc)?;
        {
            let document = self.document_mut();
            document.path = Some(path.clone());
            document.mark_saved();
        }
        self.refresh_project_files();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("saved as {path}"),
        );
        Ok(path)
    }

    pub fn reload_active(&mut self) -> io::Result<()> {
        let Some(root) = self.project_root.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound",
            ));
        };
        let path =
            self.document().path.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "document not saved yet")
            })?;
        let doc = project::read_document(&root, &path)?;
        {
            let document = self.document_mut();
            document.doc = doc;
            document.undo.clear();
            document.mark_saved();
            document.mark_dirty();
        }
        self.hydrate_documents();

        self.clamp_selection();
        self.log(LogSource::Editor, LogLevel::Info, format!("re-read {path}"));
        Ok(())
    }

    pub fn has_project(&self) -> bool {
        self.project_root.is_some()
    }

    pub fn save_suggestion(&self) -> String {
        let document = self.document();
        match &document.path {
            Some(path) => path.clone(),
            None => match &self.project_root {
                Some(root) => project::default_save_path(root, document.kind, &document.title()),
                None => format!("{}/{}", document.kind.folder(), document.title()),
            },
        }
    }

    pub(super) fn persist_scene_manifest(&mut self) {
        let Some(root) = self.project_root.clone() else {
            return;
        };
        if let Err(error) = self.scene_manifest.write(&root) {
            self.log(
                LogSource::Editor,
                LogLevel::Error,
                format!("write assets/scenes.json failed: {error}"),
            );
        }
    }

    pub fn reload_scene_manifest(&mut self) -> io::Result<()> {
        let Some(root) = self.project_root.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no project root bound",
            ));
        };
        self.scene_manifest = SceneManifest::read(&root)?;
        Ok(())
    }

    pub fn set_scene_enabled(&mut self, index: usize, enabled: bool) {
        if self.scene_manifest.set_enabled(index, enabled) {
            self.persist_scene_manifest();
        }
    }

    pub fn move_scene_entry(&mut self, index: usize, delta: i32) -> Option<usize> {
        let moved = self.scene_manifest.move_entry(index, delta);
        if moved.is_some() {
            self.persist_scene_manifest();
        }
        moved
    }

    pub fn remove_scene_entry(&mut self, index: usize) -> bool {
        let removed = self.scene_manifest.remove(index).is_some();
        if removed {
            self.persist_scene_manifest();
        }
        removed
    }

    pub fn add_scene_document(&mut self, doc_relative: &str) -> Option<usize> {
        self.project_root.as_ref()?;
        let added = self.scene_manifest.add_document(doc_relative);
        if added.is_some() {
            self.persist_scene_manifest();
        }
        added
    }

    pub fn available_scene_documents(&self) -> Vec<String> {
        let mut docs: Vec<String> = self
            .project_files
            .iter()
            .filter(|file| file.kind == DocKind::Scene)
            .map(|file| file.path.trim_start_matches("assets/").to_string())
            .filter(|relative| !self.scene_manifest.contains_document(relative))
            .collect();
        docs.sort();
        docs
    }

    pub fn filtered_assets(&self) -> Vec<&AssetInfo> {
        crate::assets::filter_assets(&self.assets, &self.asset_query)
    }

    pub fn asset_picker_candidates(&self, field: &str) -> Vec<AssetInfo> {
        let leaf = field
            .rsplit('.')
            .next()
            .unwrap_or(field)
            .to_ascii_lowercase();
        let kind = if leaf == "env_map"
            || leaf.contains("hdr")
            || leaf.contains("env")
            || leaf.contains("sky")
        {
            Some(AssetKind::Env)
        } else if leaf.contains("mesh") || leaf.contains("model") {
            Some(AssetKind::Model)
        } else if leaf.contains("sprite")
            || leaf.contains("texture")
            || leaf.contains("icon")
            || leaf.contains("tex")
        {
            Some(AssetKind::Texture)
        } else if leaf.contains("audio") || leaf.contains("sound") || leaf.contains("music") {
            Some(AssetKind::Audio)
        } else if leaf.contains("material") {
            Some(AssetKind::Material)
        } else if leaf == "font" || leaf.contains("font") {
            Some(AssetKind::Font)
        } else if leaf.contains("prefab") {
            Some(AssetKind::Prefab)
        } else if leaf.contains("scene") {
            Some(AssetKind::Scene)
        } else {
            None
        };
        let mut candidates: Vec<AssetInfo> = self
            .assets
            .iter()
            .filter(|asset| kind.is_none_or(|kind| asset.kind == kind))
            .cloned()
            .collect();
        candidates.sort_by(|a, b| a.path.cmp(&b.path));
        candidates
    }

    pub fn toggle_asset_kind(&mut self, kind: AssetKind) {
        match self.asset_query.kinds.iter().position(|item| *item == kind) {
            Some(index) => {
                self.asset_query.kinds.remove(index);
            }
            None => self.asset_query.kinds.push(kind),
        }
    }

    pub fn toggle_asset_folder(&mut self, path: &str) {
        if !self.expanded_assets.insert(path.to_string()) {
            self.expanded_assets.remove(path);
        }
    }
}
