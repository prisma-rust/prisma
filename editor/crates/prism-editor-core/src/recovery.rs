//! Exit snapshot for unsaved documents (#619).
//!
//! egui-ash calls `event_loop.exit()` in winit's `CloseRequested` first, so the editor
//! `CancelClose` + "Unsaved Changes" dialog does not take effect in time on actual window close. Rather than
//! pin hopes of preventing data loss on the dependency repo's event loop, this writes dirty documents
//! (`LoopExiting` — event loop ended but app still accessible) into the project's recovery file.
//!
//! Snapshot does **not** overwrite original scene / prefab: next launch UI asks Recover / Discard, recovered
//! document is still dirty, saved to disk only after user confirmation. File under `target/`, not in version control.

use crate::doc::SceneDoc;
use crate::workspace::DocKind;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Recovery snapshot location relative to project root.
pub const RECOVERY_DIR: &str = "target/editor-recovery";
/// Snapshot file name; all documents in one JSON array, write is atomic.
pub const RECOVERY_FILE: &str = "recovery.json";

/// One document awaiting recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveredDocument {
    /// Original document path relative to project root; `None` = never-saved Untitled document.
    pub path: Option<String>,
    pub kind: DocKind,
    pub document: SceneDoc,
}

fn recovery_path(root: &Path) -> PathBuf {
    root.join(RECOVERY_DIR).join(RECOVERY_FILE)
}

/// Atomically write snapshot. Does nothing when `documents` empty (does not delete old snapshot — deletion only by
/// Recover / Discard explicit trigger, avoiding a normal save wiping it after user ignores the hint).
pub fn write(root: &Path, documents: &[RecoveredDocument]) -> io::Result<usize> {
    if documents.is_empty() {
        return Ok(0);
    }
    let dir = root.join(RECOVERY_DIR);
    fs::create_dir_all(&dir)?;
    let path = dir.join(RECOVERY_FILE);
    let tmp = dir.join(format!("{RECOVERY_FILE}.tmp"));
    let mut text = serde_json::to_string_pretty(documents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    text.push('\n');
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        // Snapshot is the last insurance, must really flush to disk before rename.
        file.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(documents.len())
}

/// Read snapshot; returns empty list when no snapshot.
pub fn read(root: &Path) -> io::Result<Vec<RecoveredDocument>> {
    let path = recovery_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    serde_json::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Delete snapshot (along with empty recovery directory).
pub fn clear(root: &Path) -> io::Result<()> {
    let path = recovery_path(root);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // Directory may contain user / other process files, delete only when empty.
    let _ = fs::remove_dir(root.join(RECOVERY_DIR));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::SceneDoc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "prism-editor-recovery-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("assets/scenes")).unwrap();
            Self { path }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn doc(name: &str) -> SceneDoc {
        let mut doc = SceneDoc::default();
        doc.add_named_child(None, name);
        doc
    }

    #[test]
    fn round_trips_documents_and_clears() {
        let root = TempRoot::new();
        let entries = vec![
            RecoveredDocument {
                path: Some("assets/scenes/a.scene.json".to_string()),
                kind: DocKind::Scene,
                document: doc("a"),
            },
            RecoveredDocument {
                path: None,
                kind: DocKind::Prefab,
                document: doc("untitled"),
            },
        ];
        assert_eq!(write(&root.path, &entries).unwrap(), 2);
        assert!(recovery_path(&root.path).exists());

        let read_back = read(&root.path).unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(
            read_back[0].path.as_deref(),
            Some("assets/scenes/a.scene.json")
        );
        assert_eq!(read_back[0].kind, DocKind::Scene);
        assert_eq!(read_back[0].document, entries[0].document);
        assert_eq!(read_back[1].path, None);
        assert_eq!(read_back[1].kind, DocKind::Prefab);

        clear(&root.path).unwrap();
        assert!(!recovery_path(&root.path).exists());
        assert!(read(&root.path).unwrap().is_empty());
    }

    #[test]
    fn writing_no_documents_keeps_an_existing_snapshot() {
        let root = TempRoot::new();
        let entries = vec![RecoveredDocument {
            path: Some("assets/scenes/a.scene.json".to_string()),
            kind: DocKind::Scene,
            document: doc("a"),
        }];
        write(&root.path, &entries).unwrap();

        assert_eq!(write(&root.path, &[]).unwrap(), 0);
        assert_eq!(read(&root.path).unwrap().len(), 1);
    }

    #[test]
    fn missing_snapshot_reads_as_empty() {
        let root = TempRoot::new();
        assert!(read(&root.path).unwrap().is_empty());
        clear(&root.path).unwrap();
    }

    #[test]
    fn malformed_snapshot_is_an_error_not_a_panic() {
        let root = TempRoot::new();
        fs::create_dir_all(root.path.join(RECOVERY_DIR)).unwrap();
        fs::write(recovery_path(&root.path), "{ not json").unwrap();
        let error = read(&root.path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
