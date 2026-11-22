//! Asset sidecar (`.rmeta`): stable GUID + importer name (#466).
//!
//! Each source asset may have a `<path>.rmeta` alongside it (e.g.
//! `assets/textures/wood.png.rmeta`):
//!
//! ```json
//! { "guid": "0123456789abcdef0123456789abcdef", "importer": "TextureImporter" }
//! ```
//!
//! GUID is generated **once on first scan**; afterwards the sidecar travels with the file on
//! rename / move, so referrers can look it up by GUID instead of rewriting all documents when
//! a file moves. The current version only persists identity to disk and lets the editor asset
//! details display it (the `*_guid` fields in `.scene.json` are next).
//!
//! `uuid` is deliberately not added as a dependency: the editor data layer only depends on
//! serde / serde_json / log. The generator uses timestamp + process id + atomic counter + path
//! with two rounds of FNV-1a, outputting 32-bit hex, consistent with the old shape of
//! `assets::asset_guid`; it's not cryptographically random, but sufficient for in-project
//! identity.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sidecar suffix (appended to the full file name: `wood.png.rmeta`).
pub const META_SUFFIX: &str = ".rmeta";

static GUID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Canonical path of `<path>.rmeta` (relative to project root, same shape as asset path).
pub fn meta_relative_path(path: &str) -> String {
    format!("{path}{META_SUFFIX}")
}

/// `.rmeta` file content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetMeta {
    pub guid: String,
    #[serde(default)]
    pub importer: String,
}

impl AssetMeta {
    /// Read sidecar; missing file, bad JSON, or bad GUID shape all return `None`.
    pub fn read(root: &Path, relative: &str) -> Option<Self> {
        let text = fs::read_to_string(root.join(meta_relative_path(relative))).ok()?;
        let meta: AssetMeta = serde_json::from_str(&text).ok()?;
        valid_guid(&meta.guid).then_some(meta)
    }

    /// Write sidecar (parent directory must exist; write failure returns the error as-is).
    pub fn write(&self, root: &Path, relative: &str) -> io::Result<()> {
        let path = root.join(meta_relative_path(relative));
        let mut text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        text.push('\n');
        fs::write(path, text)
    }

    /// Read the existing sidecar; if missing / corrupt, generate a new GUID and write it back.
    pub fn ensure(root: &Path, relative: &str, importer: &str) -> io::Result<Self> {
        if let Some(existing) = Self::read(root, relative) {
            return Ok(existing);
        }
        let meta = AssetMeta {
            guid: new_guid(relative),
            importer: importer.to_string(),
        };
        meta.write(root, relative)?;
        Ok(meta)
    }
}

/// A valid GUID: 32-bit ASCII hex (case accepted, kept as-is on write-back).
pub fn valid_guid(guid: &str) -> bool {
    guid.len() == 32 && guid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Generate a stable 32-bit hex GUID (see module docs).
pub fn new_guid(path: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = GUID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{nanos}:{}:{counter}:{path}", std::process::id());
    let high = fnv1a(seed.as_bytes(), 0xcbf2_9ce4_8422_2325);
    let low = fnv1a(seed.as_bytes(), 0x8422_2325_cbf2_9ce4);
    format!("{high:016x}{low:016x}")
}

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
#[path = "asset_meta_tests.rs"]
mod tests;
