//! Loose asset hot reload (#495 item 2): files under `assets/` not packed changed, auto repack scene package.
//!
//! Runtime reads assets only from `.pak`, so changing a texture / swapping HDR / newly imported mesh must re-
//! cook package to enter memory. Editor here does minimal closed loop:
//!
//! 1. Poll version stamps (mtime + size) of **authoring assets** under `assets/` (excluding generated `.pak` / `.rscn` / `.spv` etc.,
//!    and `.scene.json` / `.prefab.json` --- they are auto-saved by document, repacking meaningless);
//! 2. After change silently `DEBOUNCE`, avoid continuous trigger when saving large file;
//! 3. Trigger existing `BuildKind::ScenePack` once; after pack `EngineHost::poll_asset_changes`
//!    existing link reopens `.pak` and `invalidate_assets`, no need to exit Play.
//!
//! Scan is pure filesystem op, test with temp dir directly; time injected by caller, convenient to test debounce.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use prism_editor_core::project::{file_stamp_at, FileStamp};

/// Wait this long after change before packing (covers multi-file save / large file chunked write).
pub const DEBOUNCE: Duration = Duration::from_millis(800);

/// Generated products / editor docs: not participate in loose asset scan.
///
/// - `.pak` / `.pak.meta.json`: packaging output itself, including causes infinite trigger;
/// - `.rscn`: `cook` single-scene product;
/// - `.spv`: shader compile product (changing shader needs recompile, not asset packaging);
/// - `.scene.json` / `.prefab.json`: document auto-save writes disk, repacking no benefit.
fn is_watched(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".pak")
        || name.ends_with(".pak.meta.json")
        || name.ends_with(".rscn")
        || name.ends_with(".spv")
        || name.ends_with(".scene.json")
        || name.ends_with(".prefab.json")
    {
        return false;
    }
    // Hidden files / temp files (vim, VSCode atomic save, system thumbnail cache).
    if name.starts_with('.') || name.ends_with('~') || name.ends_with(".swp") {
        return false;
    }
    true
}

fn skip_dir(name: &str) -> bool {
    matches!(name, ".git" | "target" | "node_modules" | ".svn" | ".hg")
}

/// Scan `(path, mtime)` of all watched files under `assets/`.
///
/// Path is **relative to project root** (consistent with `.pak` / manifest writing), so changing machine / changing
/// absolute path does not invalidate whole snapshot.
fn scan(root: &Path) -> BTreeMap<PathBuf, FileStamp> {
    let mut out = BTreeMap::new();
    let assets = root.join("assets");
    collect(&assets, root, &mut out);
    out
}

fn collect(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, FileStamp>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            if !skip_dir(&name.to_string_lossy()) {
                collect(&path, root, out);
            }
            continue;
        }
        if !file_type.is_file() || !is_watched(&path) {
            continue;
        }
        let Some(stamp) = file_stamp_at(&path) else {
            continue;
        };
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        out.insert(relative, stamp);
    }
}

/// Loose asset poller: snapshot + debounce state.
#[derive(Debug, Default)]
pub struct LooseAssetWatcher {
    snapshot: BTreeMap<PathBuf, FileStamp>,
    /// Time of most recent change detected; trigger only after quiet [`DEBOUNCE`].
    pending: Option<Instant>,
}

impl LooseAssetWatcher {
    pub fn new(root: &Path) -> Self {
        Self {
            snapshot: scan(root),
            pending: None,
        }
    }

    /// Poll once; returns `true` (once) after change quiet for [`DEBOUNCE`].
    ///
    /// `now` injected by caller, test can precisely control debounce timing.
    pub fn poll(&mut self, root: &Path, now: Instant) -> bool {
        let current = scan(root);
        if current != self.snapshot {
            self.snapshot = current;
            self.pending = Some(now);
            return false;
        }
        match self.pending {
            Some(at) if now.saturating_duration_since(at) >= DEBOUNCE => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }

    /// Re-align snapshot (used after build just wrote pack / switch project), discard untriggered debounce.
    pub fn resync(&mut self, root: &Path) {
        self.snapshot = scan(root);
        self.pending = None;
    }

    /// Whether currently waiting for debounce end (for test).
    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prism-loose-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("assets/sprites")).unwrap();
        dir
    }

    fn touch(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn generated_artifacts_and_documents_are_not_watched() {
        let root = temp_dir("filter");
        for name in [
            "scenes.pak",
            "scenes.pak.meta.json",
            "scenes/a.rscn",
            "shaders/post.frag.spv",
            "scenes/lake.scene.json",
            "prefabs/boat.prefab.json",
            ".hidden.png",
            "sprites/boat.png~",
        ] {
            touch(&root.join("assets").join(name), b"x");
        }
        touch(&root.join("assets/sprites/boat.png"), b"x");
        touch(&root.join("assets/models/boat.gltf"), b"x");

        let scan = scan(&root);
        let mut names: Vec<String> = scan
            .keys()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["assets/models/boat.gltf", "assets/sprites/boat.png"]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_change_fires_once_after_the_debounce() {
        let root = temp_dir("debounce");
        touch(&root.join("assets/sprites/boat.png"), b"v1");
        let mut watcher = LooseAssetWatcher::new(&root);
        let start = Instant::now();

        // No change: never trigger.
        assert!(!watcher.poll(&root, start));
        assert!(!watcher.poll(&root, start + DEBOUNCE * 2));

        // File changed -> only record pending, do not trigger immediately.
        touch(&root.join("assets/sprites/boat.png"), b"v2-longer");
        assert!(!watcher.poll(&root, start));
        assert!(watcher.is_pending());
        assert!(!watcher.poll(&root, start + DEBOUNCE / 2));

        // Quiet for DEBOUNCE -> trigger once, then return to idle.
        assert!(watcher.poll(&root, start + DEBOUNCE + Duration::from_millis(1)));
        assert!(!watcher.is_pending());
        assert!(!watcher.poll(&root, start + DEBOUNCE * 3));

        // Continuous writes re-time debounce.
        touch(&root.join("assets/sprites/boat.png"), b"v3-a");
        assert!(!watcher.poll(&root, start));
        touch(&root.join("assets/sprites/boat.png"), b"v4-longer");
        assert!(!watcher.poll(&root, start + Duration::from_millis(600)));
        assert!(!watcher.poll(&root, start + DEBOUNCE));
        assert!(watcher.poll(&root, start + Duration::from_millis(600) + DEBOUNCE));

        // New file / deletion also counts as change.
        touch(&root.join("assets/models/new.gltf"), b"x");
        assert!(!watcher.poll(&root, start));
        assert!(watcher.poll(&root, start + DEBOUNCE + Duration::from_millis(2)));
        std::fs::remove_file(root.join("assets/models/new.gltf")).unwrap();
        assert!(!watcher.poll(&root, start));
        assert!(watcher.poll(&root, start + DEBOUNCE + Duration::from_millis(3)));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resync_discards_a_pending_trigger() {
        let root = temp_dir("resync");
        touch(&root.join("assets/sprites/boat.png"), b"v1");
        let mut watcher = LooseAssetWatcher::new(&root);
        let start = Instant::now();
        touch(&root.join("assets/sprites/boat.png"), b"v2-longer");
        assert!(!watcher.poll(&root, start));
        assert!(watcher.is_pending());
        watcher.resync(&root);
        assert!(!watcher.is_pending());
        assert!(!watcher.poll(&root, start + DEBOUNCE * 2));
        std::fs::remove_dir_all(&root).ok();
    }
}
