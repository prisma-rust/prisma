//! Player save (#984): a minimal persistence layer that "saves on purchase, opens on restart".
//!
//! Engine built-in `scene_state.json` only stores scene (camera / transform / light), player progress
//! (fish caught, money earned, unlocks) has nowhere to go, each project had to write own file — so each project
//! handled "where to save, how to write atomically, how to upgrade" separately. Here unified:
//!
//! - Location decided by [`user_data_dir`] (Windows `%APPDATA%`, macOS
//!   `Application Support`, Linux `XDG_DATA_HOME`), Android host points
//!   `PATH` to `filesDir` ([`ENV_OVERRIDE`]).
//! - Content is a **string key-value table** ([`UserData`]) with `version`. Engine does not understand
//!   business schema, project serializes structure into strings itself, checks version on upgrade.
//! - Writes are **atomic** (temp file + rename), crash midway leaves no half save.
//! - Reading version higher than [`USER_DATA_VERSION`] **does not overwrite**: returns error and keeps original file,
//!   avoiding new-version save wiped by old client.
//!
//! Usage (project side):
//!
//! ```ignore
//! world.insert_resource(UserStore::open(save_path("my-game")));
//! // Read
//! let caught = store.get("caught").unwrap_or("0");
//! // Write (memory + disk)
//! store.set("caught", "3");
//! store.flush()?;
//! ```

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Save format version. Bump only when field semantics change (new keys do not need).
pub const USER_DATA_VERSION: u32 = 1;

/// Env var overriding user data root directory (Android host / portable / tests).
pub const ENV_OVERRIDE: &str = "PRISMA_USER_DATA";

/// Save filename: under each project own directory.
pub const FILE_NAME: &str = "player_data.json";

/// Save file content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserData {
    /// [`USER_DATA_VERSION`]; missing treated as 1 (first version did not write version).
    #[serde(default = "default_version")]
    pub version: u32,
    /// String key-value table. Keys by project convention (`"caught"` / `"coins"` / `"quest.mayor"`).
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    1
}

impl Default for UserData {
    fn default() -> Self {
        Self {
            version: USER_DATA_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl UserData {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.insert(key.into(), value.into());
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.entries.remove(key)
    }

    /// Parse file content.
    ///
    /// Save newer than current engine returns `Err` — caller **must not write back**, otherwise new-version
    /// player downgrading to old client loses save.
    pub fn parse(text: &str) -> Result<Self, String> {
        let data: Self = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if data.version > USER_DATA_VERSION {
            return Err(format!(
                "save data version {} is newer than this build ({USER_DATA_VERSION})",
                data.version
            ));
        }
        Ok(data)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// User data root directory (one subdir per project).
///
/// Order: `PRISMA_USER_DATA` → platform conventional directory. Returned path **not guaranteed to exist**,
/// created on demand by [`UserStore::flush`].
pub fn user_data_dir(project: &str) -> PathBuf {
    let root = if let Some(override_dir) = std::env::var_os(ENV_OVERRIDE) {
        PathBuf::from(override_dir)
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Prisma")
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Prisma")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local/share")
            })
            .join("prisma")
    };
    root.join(sanitize(project))
}

/// Save file path for a project.
pub fn save_path(project: &str) -> PathBuf {
    user_data_dir(project).join(FILE_NAME)
}

/// Project name may contain slashes / colons, replaced before writing to disk to avoid escaping user data dir.
fn sanitize(project: &str) -> String {
    let cleaned: String = project
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned
    }
}

/// Runtime save state: in-memory key-value table + which file it belongs to.
///
/// Attached to `World` as ECS resource, so `GameWorld` (host backend) and engine
/// systems can read / write directly, no extra channel needed.
#[derive(Debug, Clone, Default)]
pub struct UserStore {
    path: Option<PathBuf>,
    data: UserData,
    /// Read a higher-version save: read-only, not overwritten.
    read_only: bool,
    dirty: bool,
}

impl UserStore {
    /// Memory only (not on disk): unit tests / hosts without file system.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Bind a save file and read (missing file = empty save, not an error).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut store = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match UserData::parse(&text) {
                Ok(data) => store.data = data,
                Err(error) => {
                    log::warn!(
                        "player data {}: {error}; keeping the file untouched",
                        path.display()
                    );
                    store.read_only = true;
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                log::warn!("player data {}: {error}", path.display());
                store.read_only = true;
            }
        }
        store
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn data(&self) -> &UserData {
        &self.data
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        if self.read_only {
            log::warn!("player data is read-only (newer version on disk); ignoring write");
            return;
        }
        self.data.set(key, value);
        self.dirty = true;
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        if self.read_only {
            return None;
        }
        let removed = self.data.remove(key);
        if removed.is_some() {
            self.dirty = true;
        }
        removed
    }

    /// Whether there are unflushed changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Write to disk (temp file + rename, power failure leaves no half save).
    ///
    /// No bound path, read-only, or no changes returns `Ok(false)` directly.
    pub fn flush(&mut self) -> io::Result<bool> {
        if self.read_only || !self.dirty {
            return Ok(false);
        }
        let Some(path) = self.path.clone() else {
            self.dirty = false;
            return Ok(false);
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, self.data.to_json())?;
        std::fs::rename(&temp, &path)?;
        self.dirty = false;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "user_data_tests.rs"]
mod user_data_tests;
