//! # prism-asset::runtime
//!
//! Lightweight runtime resource asset loader, depends only on `prism-asset` `core` and `package` modules.
//!
//! This module is the game code-only consumer-facing resource reading API. It provides [`ResourceManager`],
//! used to load `.pak` package and query/resolve [`Handle<T>`] reference via [`AssetId`].
//!
//! ## Design constraints
//!
//! - Runtime has no source file path---all access goes through [`AssetId`].
//! - Does not depend on build tool (`prism-cooker` importer/cooker/db etc.).
//! - Supports sync + async loading.
//! - Controls memory budget via LRU eviction (phase 3).
//! - Supports hot reload via file watcher ([`HotReloadWatcher`]), always available at runtime.
//! - Large resource streaming read (phase 3, feature `streaming`).

use crate::core::{AnyHandle, AssetId, AssetType, Handle};
use crate::package::{PackageError, PackageReader};
#[cfg(feature = "streaming")]
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

mod hot_reload;

pub use hot_reload::{HotReloadError, HotReloadEvent, HotReloadWatcher};

pub mod typed;

pub use typed::{
    Anim2DAsset, AnimClipAsset, AnimGraphAsset, AtlasAsset, MaterialAsset, MeshAsset, SceneAsset,
    ShaderAsset, SkeletonAsset, SkinnedMeshAsset, StateMachineAsset, TextureAsset, TilemapAsset,
    VirtualGeoAsset,
};

pub use crate::formats::{
    Anim2DFrame, Anim2DTag, AtlasSpriteEntry, MergedCollisionBox, Ranim2DInfo, RatlInfo, RmatInfo,
    RmesInfo, RtilemapInfo, TilemapLayer, TilemapObject,
};

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Asset not loaded: {0}")]
    NotLoaded(AssetId),

    #[error("Asset not found in any package: {0}")]
    NotFound(AssetId),

    #[error("Package error: {0}")]
    Package(#[from] crate::package::PackageError),

    #[error("Handle generation mismatch for slot {index}: expected {expected}, got {got}")]
    GenerationMismatch { index: u32, expected: u32, got: u32 },

    #[error("Slot {index} is empty")]
    SlotEmpty { index: u32 },

    #[error("Asset type mismatch: expected {expected:?}, got {got:?}")]
    TypeMismatch {
        expected: &'static str,
        got: AssetType,
    },

    #[error("Failed to deserialize {asset_type:?}: {reason}")]
    DeserializeFailed {
        asset_type: AssetType,
        reason: String,
    },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Memory budget would be exceeded ({current} + {needed} > {max})")]
    OutOfMemory { current: u64, needed: u64, max: u64 },

    #[error("Cannot unpin asset {0}: pin count is already zero")]
    PinUnderflow(AssetId),
}

/// One slot in the runtime slot array.
#[derive(Clone)]
struct Slot {
    generation: u32,
    asset_id: AssetId,
    asset_type: AssetType,
    /// The raw cooked data, as stored in the .pak (uncompressed).
    data: Option<Vec<u8>>,
    /// Last access timestamp (for LRU eviction). `Cell` allows read-only
    /// accessors (`get_raw`, `read_stream`) refresh recency without `&mut`.
    last_access: Cell<Instant>,
    /// Size of the asset data in bytes (for memory tracking)
    size_bytes: u64,
    /// Load sequence stamp (for FIFO eviction), assigned when data is cached.
    load_epoch: u64,
    /// Number of active pins protecting this slot from automatic eviction.
    pin_count: u32,
}

impl Slot {
    fn new() -> Self {
        Self {
            generation: 0,
            asset_id: AssetId::from_raw(0),
            asset_type: AssetType::Unknown,
            data: None,
            last_access: Cell::new(Instant::now()),
            size_bytes: 0,
            load_epoch: 0,
            pin_count: 0,
        }
    }
}

/// The eviction policy when memory budget is exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EvictionPolicy {
    /// No automatic eviction (caller must manage manually).
    None,
    /// Evict least-recently-accessed assets first.
    #[default]
    Lru,
    /// Evict oldest-loaded assets first.
    Fifo,
}

/// Tracks memory usage and enforces budget via eviction.
#[derive(Debug, Clone)]
struct MemoryTracker {
    /// Maximum memory budget in bytes (0 = unlimited).
    budget: u64,
    /// Current memory usage in bytes.
    current: u64,
    /// Eviction policy.
    policy: EvictionPolicy,
}

impl MemoryTracker {
    fn new() -> Self {
        Self {
            budget: 0,
            current: 0,
            policy: EvictionPolicy::default(),
        }
    }

    fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    fn set_policy(&mut self, policy: EvictionPolicy) {
        self.policy = policy;
    }

    fn can_fit(&self, bytes: u64) -> bool {
        self.budget == 0 || self.current + bytes <= self.budget
    }

    fn account_add(&mut self, bytes: u64) {
        self.current += bytes;
    }

    fn account_remove(&mut self, bytes: u64) {
        self.current = self.current.saturating_sub(bytes);
    }

    fn usage_ratio(&self) -> f32 {
        if self.budget == 0 {
            0.0
        } else {
            self.current as f32 / self.budget as f32
        }
    }
}

/// Registered `.pak`: keeps source path together.
///
/// `on_pak_changed` only knows which reader came from which file, can then replace **stale reader**;
/// otherwise after hot reload new `load` would still read old bytes.
struct LoadedPackage {
    /// `None` = register from memory bytes (`ResourceManager::load_package_bytes`), no file on disk to replace.
    ///
    /// Only `on_pak_changed` reads it (hot reload relies on it to find reader to replace).
    path: Option<PathBuf>,
    reader: PackageReader,
}

/// The central runtime resource manager.
///
/// Manages a slot array of loaded assets, indexed by [`Handle<T>`].
/// Packages are loaded and their assets registered. The manager owns the
/// loaded data and provides generation-counted handles for safe access.
///
/// ## Phase 3 features
///
/// - **Memory budget**: call [`set_memory_budget()`](ResourceManager::set_memory_budget)
///   to cap total loaded bytes LRU or FIFO eviction.
/// - **Hot reload**: use [`HotReloadWatcher`] to watch `.pak` files, then hand each
///   event to [`ResourceManager::on_pak_changed`].
/// - **Streaming**: call [`read_stream()`](ResourceManager::read_stream) to
///   iterate over an asset's data in fixed-size chunks (zero-copy within a
///   loaded package).
///
pub struct ResourceManager {
    /// Slot array indexed by handle index.
    slots: Vec<Slot>,
    /// Map from AssetId -> slot index.
    id_map: HashMap<AssetId, u32>,
    /// Map from source-relative path -> AssetId, populated by
    /// [`Self::load_path_manifest`]. Lets the engine resolve scene
    /// `mesh_path`/`material_path` strings to runtime `AssetId`s. Empty when
    /// no manifest has been loaded (path-based lookup is unavailable).
    path_map: HashMap<String, AssetId>,
    /// Loaded package readers.
    packages: Vec<LoadedPackage>,
    /// Next free slot index.
    next_slot: u32,
    /// Memory tracking.
    memory: MemoryTracker,
    /// Monotonic load counter (for FIFO eviction).
    load_epoch: u64,
}

impl ResourceManager {
    /// Create a new empty resource manager.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            id_map: HashMap::new(),
            path_map: HashMap::new(),
            packages: Vec::new(),
            next_slot: 1,
            memory: MemoryTracker::new(),
            load_epoch: 0,
        }
    }

    /// Set the memory budget in bytes (0 = unlimited).
    ///
    /// When the budget would be exceeded during `load`, assets will be
    /// evicted according to the eviction policy.
    pub fn set_memory_budget(&mut self, bytes: u64) {
        self.memory.set_budget(bytes);
    }

    /// Current memory budget (0 = unlimited).
    pub fn memory_budget(&self) -> u64 {
        self.memory.budget
    }

    /// Current memory usage (sum of loaded asset data sizes).
    pub fn memory_usage(&self) -> u64 {
        self.memory.current
    }

    /// Ratio of used to budgeted memory (0.0 – 1.0). Returns 0.0 if
    /// the budget is unlimited.
    pub fn memory_usage_ratio(&self) -> f32 {
        self.memory.usage_ratio()
    }

    /// Set the eviction policy.
    pub fn set_eviction_policy(&mut self, policy: EvictionPolicy) {
        self.memory.set_policy(policy);
    }

    /// Current eviction policy.
    pub fn eviction_policy(&self) -> EvictionPolicy {
        self.memory.policy
    }

    /// Try to free at least `target_bytes` by evicting assets.
    /// Returns bytes actually freed.
    pub fn evict(&mut self, target_bytes: u64) -> u64 {
        if target_bytes == 0 || self.memory.policy == EvictionPolicy::None {
            return 0;
        }

        let mut freed: u64 = 0;

        let mut candidates: Vec<(u32, Instant, u64, u64)> = self
            .id_map
            .values()
            .filter_map(|&idx| {
                let slot = &self.slots[idx as usize];
                if slot.data.is_some() && slot.pin_count == 0 {
                    Some((
                        idx,
                        slot.last_access.get(),
                        slot.size_bytes,
                        slot.load_epoch,
                    ))
                } else {
                    None
                }
            })
            .collect();

        match self.memory.policy {
            EvictionPolicy::Lru => {
                candidates.sort_by_key(|&(_, time, _, _)| time);
            }
            EvictionPolicy::Fifo => {
                candidates.sort_by_key(|&(_, _, _, epoch)| epoch);
            }
            EvictionPolicy::None => return 0,
        }

        for (idx, _time, size, _epoch) in &candidates {
            if freed >= target_bytes {
                break;
            }
            let slot = &mut self.slots[*idx as usize];
            if slot.pin_count > 0 {
                continue;
            }
            if slot.data.take().is_some() {
                self.memory.account_remove(*size);
                freed += *size;
                tracing::debug!("Evicted slot {} (freed {} bytes)", idx, size);
            }
        }

        freed
    }

    /// Load and register all assets from a `.pak` file.
    pub fn load_package(&mut self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let path = path.as_ref().to_path_buf();
        let reader = PackageReader::open(&path)?;
        self.register_package(LoadedPackage {
            path: Some(path),
            reader,
        });
        Ok(())
    }

    /// Load asset from `.pak` bytes in memory, for Android asset/embedded host use.
    pub fn load_package_bytes(&mut self, bytes: &[u8]) -> Result<(), RuntimeError> {
        let reader = PackageReader::from_bytes(bytes)?;
        self.register_package(LoadedPackage { path: None, reader });
        Ok(())
    }

    /// Load and register all assets from a `.pak` file (async).
    #[cfg(feature = "async-io")]
    pub async fn load_package_async(
        &mut self,
        path: impl AsRef<Path> + Send,
    ) -> Result<(), RuntimeError> {
        let path = path.as_ref().to_path_buf();
        let reader = PackageReader::open_async(&path).await?;
        self.register_package(LoadedPackage {
            path: Some(path),
            reader,
        });
        Ok(())
    }

    /// Load a path manifest (`.pak.meta.json` written by `prism-cook build`
    /// so the engine can resolve source-relative asset paths to runtime
    /// `AssetId`s.
    ///
    /// The manifest is the **only** runtime source of path->id mapping: the
    /// `.pak` binary itself stores only `AssetId`s (paths are an editor-side
    /// concept). Without this call, [`Self::id_by_path`] always returns
    /// `None`.
    ///
    /// The manifest format (produced by `cmd_build`) is a JSON object with an
    /// `assets` array, each entry having `id` (hex string) and `path` (string).
    pub fn load_path_manifest(&mut self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let text = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: format!("read path manifest: {e}"),
            }
        })?;
        self.load_path_manifest_from_str(&text)
    }

    /// Same as [`Self::load_path_manifest`] but parses an in-memory JSON string.
    pub fn load_path_manifest_from_str(&mut self, text: &str) -> Result<(), RuntimeError> {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|e| RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: format!("parse path manifest JSON: {e}"),
            })?;

        let assets = json
            .get("assets")
            .and_then(|a| a.as_array())
            .ok_or_else(|| RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: "manifest missing 'assets' array".into(),
            })?;

        let mut added = 0usize;
        for entry in assets {
            let id_str = entry.get("id").and_then(|v| v.as_str());
            let path = entry.get("path").and_then(|v| v.as_str());
            let (Some(id_str), Some(path)) = (id_str, path) else {
                continue;
            };
            let raw = if let Some(stripped) = id_str.strip_prefix("0x") {
                u64::from_str_radix(stripped, 16)
            } else {
                u64::from_str_radix(id_str, 16)
            }
            .map_err(|e| RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: format!("manifest asset id '{id_str}' is not hex: {e}"),
            })?;
            let id = AssetId::from_raw(raw);
            self.path_map.insert(path.to_owned(), id);
            added += 1;
        }

        tracing::info!("Loaded path manifest: {} entries", added);
        Ok(())
    }

    /// Resolve a source-relative asset path to its runtime `AssetId`.
    ///
    /// Returns `None` when the path isn't registered in any mounted package.
    ///
    /// Checks the explicit `.pak.meta.json` `path_map` first, then falls back to
    /// deterministic [`AssetId::from_path`] lookup against mounted package records,
    /// allowing shipping builds to resolve asset paths without an external
    /// `.pak.meta.json` file.
    pub fn id_by_path(&self, path: &str) -> Option<AssetId> {
        if let Some(&id) = self.path_map.get(path) {
            return Some(id);
        }
        let id = AssetId::from_path(path);
        self.id_map.contains_key(&id).then_some(id)
    }

    /// Register an already-open package reader.
    fn register_package(&mut self, package: LoadedPackage) {
        let asset_count = package.reader.asset_count();
        for record in package.reader.records() {
            let asset_id = AssetId::from_raw(record.id);
            if self.id_map.contains_key(&asset_id) {
                tracing::warn!("Duplicate asset ID in package: {asset_id}");
                continue;
            }

            let index = self.next_slot;
            self.next_slot += 1;

            if index as usize >= self.slots.len() {
                self.slots.resize(index as usize + 1, Slot::new());
            }
            self.slots[index as usize] = Slot {
                generation: 0,
                asset_id,
                asset_type: AssetType::from_u32(record.type_id),
                data: None,
                last_access: Cell::new(Instant::now()),
                size_bytes: record.size,
                load_epoch: 0,
                pin_count: 0,
            };
            self.id_map.insert(asset_id, index);
        }

        self.packages.push(package);
        tracing::info!(
            "Registered {} assets from package (total slots: {})",
            asset_count,
            self.id_map.len()
        );
    }

    /// Check if an asset is registered (without loading its data).
    pub fn is_registered(&self, id: AssetId) -> bool {
        self.id_map.contains_key(&id)
    }

    /// Number of registered assets.
    pub fn asset_count(&self) -> usize {
        self.id_map.len()
    }

    /// Number of loaded packages.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Get the [`AssetType`] of a registered asset.
    pub fn asset_type(&self, id: AssetId) -> Option<AssetType> {
        self.id_map
            .get(&id)
            .map(|&idx| self.slots[idx as usize].asset_type)
    }

    /// Check whether an asset is marked for streaming in any loaded package.
    pub fn is_streamed(&self, id: AssetId) -> bool {
        self.packages
            .iter()
            .any(|package| package.reader.is_asset_streamed(id))
    }

    /// Load an asset and all its dependencies recursively.
    ///
    /// Assets are loaded in topological order (dependencies first).
    /// Returns the handle to the requested root asset.
    pub fn load_with_deps<T: Asset + 'static>(
        &mut self,
        id: AssetId,
    ) -> Result<Handle<T>, RuntimeError> {
        self.load_deps_recursive(id)?;

        self.load(id)
    }

    /// Load an asset and all its dependencies recursively, then
    /// return raw bytes. Useful for assets that aren't `Asset`-typed.
    pub fn load_with_deps_raw(&mut self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        self.load_deps_recursive(id)?;
        self.load_raw_bytes(id)
    }

    /// Recursively resolve and load all dependencies of an asset.
    ///
    /// Uses a DFS that tracks the current path for cycle detection.
    /// Returns success when all deps are loaded.
    fn load_deps_recursive(&mut self, id: AssetId) -> Result<(), RuntimeError> {
        let mut visited: HashMap<AssetId, bool> = HashMap::new();
        let mut load_order: Vec<AssetId> = Vec::new();

        self.dfs_deps(id, &mut visited, &mut load_order)?;

        for &dep_id in &load_order {
            if self.get_raw_bytes(dep_id).is_err() {
                self.load_raw_bytes(dep_id)?;
            }
        }
        Ok(())
    }

    fn dfs_deps(
        &self,
        id: AssetId,
        visited: &mut HashMap<AssetId, bool>,
        load_order: &mut Vec<AssetId>,
    ) -> Result<(), RuntimeError> {
        match visited.get(&id) {
            Some(&true) => return Ok(()),
            Some(&false) => {
                tracing::warn!("Dependency cycle detected involving {id}");
                return Ok(());
            }
            None => {}
        }

        visited.insert(id, false);

        for package in &self.packages {
            let reader = &package.reader;
            if let Some(record) = reader.find_record(id) {
                let deps_raw = reader.dependencies(record);
                for &dep_raw in deps_raw {
                    let dep_id = AssetId::from_raw(dep_raw);
                    self.dfs_deps(dep_id, visited, load_order)?;
                }
                break;
            }
        }

        visited.insert(id, true);
        load_order.push(id);
        Ok(())
    }

    /// Find a slot index for an asset ID.
    fn slot_index(&self, id: AssetId) -> Option<u32> {
        self.id_map.get(&id).copied()
    }

    /// Load raw bytes for an asset without going through `Asset` trait.
    fn load_raw_bytes(&mut self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;

        {
            let slot = &self.slots[slot_index as usize];
            if let Some(ref data) = slot.data {
                slot.last_access.set(Instant::now());
                return Ok(data.clone());
            }
        }

        let data = self.read_from_packages(id)?;

        let size = data.len() as u64;
        if !self.memory.can_fit(size) {
            if self.memory.policy != EvictionPolicy::None {
                let freed = self.evict(size + 1024 * 1024);
                if !self.memory.can_fit(size) {
                    return Err(RuntimeError::OutOfMemory {
                        current: self.memory.current,
                        needed: size,
                        max: self.memory.budget,
                    });
                }
                tracing::debug!("Evicted {freed} bytes to make room for {size}");
            } else {
                return Err(RuntimeError::OutOfMemory {
                    current: self.memory.current,
                    needed: size,
                    max: self.memory.budget,
                });
            }
        }

        let epoch = self.load_epoch;
        {
            let slot = &mut self.slots[slot_index as usize];
            slot.data = Some(data.clone());
            slot.last_access.set(Instant::now());
            slot.size_bytes = size;
            slot.generation += 1;
            slot.load_epoch = epoch;
        }
        self.memory.account_add(size);
        self.load_epoch += 1;

        Ok(data)
    }

    /// Load an asset by ID and return raw bytes.
    fn get_raw_bytes(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &self.slots[slot_index as usize];
        let data = slot.data.as_ref().ok_or(RuntimeError::NotLoaded(id))?;
        slot.last_access.set(Instant::now());
        Ok(data.clone())
    }

    /// Load an asset by ID and return a typed handle.
    ///
    /// First access reads the data from the .pak and caches it. Subsequent
    /// calls return immediately.
    pub fn load<T: Asset + 'static>(&mut self, id: AssetId) -> Result<Handle<T>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;

        {
            let slot = &mut self.slots[slot_index as usize];
            if let Some(data) = slot.data.as_ref() {
                T::from_bytes(data).map_err(|_| RuntimeError::TypeMismatch {
                    expected: std::any::type_name::<T>(),
                    got: slot.asset_type,
                })?;
                slot.last_access.set(Instant::now());
                return Ok(Handle::new(slot_index, slot.generation));
            }
        }

        let data = self.read_from_packages(id)?;

        let _asset = T::from_bytes(&data).map_err(|_e| RuntimeError::TypeMismatch {
            expected: std::any::type_name::<T>(),
            got: self.slots[slot_index as usize].asset_type,
        })?;

        let size = data.len() as u64;
        if !self.memory.can_fit(size) {
            if self.memory.policy != EvictionPolicy::None {
                let _freed = self.evict(size + 1024 * 1024);
                if !self.memory.can_fit(size) {
                    return Err(RuntimeError::OutOfMemory {
                        current: self.memory.current,
                        needed: size,
                        max: self.memory.budget,
                    });
                }
            } else {
                return Err(RuntimeError::OutOfMemory {
                    current: self.memory.current,
                    needed: size,
                    max: self.memory.budget,
                });
            }
        }

        let epoch = self.load_epoch;
        {
            let slot = &mut self.slots[slot_index as usize];
            slot.data = Some(data);
            slot.last_access.set(Instant::now());
            slot.size_bytes = size;
            slot.generation += 1;
            slot.load_epoch = epoch;
        }
        self.memory.account_add(size);
        self.load_epoch += 1;

        let gen = self.slots[slot_index as usize].generation;
        Ok(Handle::new(slot_index, gen))
    }

    /// Read raw bytes from loaded packages for a given asset ID.
    fn read_from_packages(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        for package in &self.packages {
            if let Some(data) = package.reader.read_asset_data(id)? {
                return Ok(data);
            }
        }
        Err(RuntimeError::NotFound(id))
    }

    /// Read a byte sub-range `[sub_offset .. sub_offset + length]` from an asset.
    ///
    /// For uncompressed or `FLAG_STREAMED` `.pak` entries (such as 64 KiB `RCLD`
    /// geometry pages), this fetches only the requested span without loading the
    /// full asset into `ResourceManager`.
    pub fn read_asset_range(
        &self,
        id: AssetId,
        sub_offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, RuntimeError> {
        if let Some(slot_index) = self.slot_index(id) {
            let slot = &self.slots[slot_index as usize];
            if let Some(data) = slot.data.as_ref() {
                let sub_end = sub_offset.checked_add(length as u64).ok_or_else(|| {
                    RuntimeError::Package(PackageError::Truncated(
                        "asset sub-range overflows u64".into(),
                    ))
                })?;
                if sub_end > data.len() as u64 {
                    return Err(RuntimeError::Package(PackageError::Truncated(format!(
                        "asset sub-range {sub_end} exceeds cached asset size {}",
                        data.len()
                    ))));
                }
                slot.last_access.set(Instant::now());
                let start_idx = sub_offset as usize;
                let end_idx = sub_end as usize;
                return Ok(data[start_idx..end_idx].to_vec());
            }
        }

        for package in &self.packages {
            if let Some(bytes) = package.reader.read_asset_range(id, sub_offset, length)? {
                return Ok(bytes);
            }
        }
        Err(RuntimeError::NotFound(id))
    }

    /// Read an asset's data in chunks (streaming).
    ///
    /// Returns an iterator yielding `Vec<u8>` chunks of at most
    /// `chunk_size` bytes. This avoids loading the entire asset into
    /// memory at once. The asset is not cached in the slot — it is
    /// streamed directly from the package.
    ///
    /// Requires feature `streaming` (enabled by default).
    ///
    /// Returns `None` if the asset is not registered or its data cannot
    /// be read.
    #[cfg(feature = "streaming")]
    pub fn read_stream(
        &self,
        id: AssetId,
        chunk_size: usize,
    ) -> Option<impl Iterator<Item = Vec<u8>> + '_> {
        let chunk_size = chunk_size.max(1);

        if let Some(slot_index) = self.slot_index(id) {
            let slot = &self.slots[slot_index as usize];
            if let Some(data) = slot.data.as_ref() {
                slot.last_access.set(Instant::now());
                return Some(StreamIter::Slice(SliceStreamIter {
                    data: Cow::Borrowed(data.as_slice()),
                    pos: 0,
                    chunk_size,
                }));
            }
        }

        for package in &self.packages {
            if let Some(record) = package.reader.find_record(id) {
                let raw = match package.reader.asset_record_raw_slice(record) {
                    Ok(slice) => Cow::Borrowed(slice),
                    Err(PackageError::NotInMemory) => {
                        match package.reader.asset_record_raw_bytes(record) {
                            Ok(bytes) => Cow::Owned(bytes),
                            Err(_) => continue,
                        }
                    }
                    Err(_) => continue,
                };
                if let Some(slot_index) = self.slot_index(id) {
                    self.slots[slot_index as usize]
                        .last_access
                        .set(Instant::now());
                }
                if record.is_compressed() {
                    let source: Box<dyn std::io::Read + '_> = Box::new(std::io::Cursor::new(raw));
                    if let Ok(decoder) = zstd::Decoder::new(source) {
                        return Some(StreamIter::Compressed(CompressedStreamIter {
                            decoder,
                            chunk_size,
                            total_size: record.size as usize,
                            read_so_far: 0,
                        }));
                    }
                } else {
                    return Some(StreamIter::Slice(SliceStreamIter {
                        data: raw,
                        pos: 0,
                        chunk_size,
                    }));
                }
            }
        }

        None
    }

    /// Get a typed reference to already-loaded asset data.
    pub fn get<T: Asset + 'static>(&mut self, handle: Handle<T>) -> Result<T, RuntimeError> {
        let index = handle.index() as usize;
        if index >= self.slots.len() {
            return Err(RuntimeError::SlotEmpty {
                index: handle.index(),
            });
        }
        let slot = &mut self.slots[index];
        if slot.generation != handle.generation() {
            return Err(RuntimeError::GenerationMismatch {
                index: handle.index(),
                expected: handle.generation(),
                got: slot.generation,
            });
        }
        slot.last_access.set(Instant::now());
        let data = slot
            .data
            .as_ref()
            .ok_or(RuntimeError::NotLoaded(slot.asset_id))?;
        T::from_bytes(data).map_err(|_| RuntimeError::TypeMismatch {
            expected: std::any::type_name::<T>(),
            got: slot.asset_type,
        })
    }

    /// Get raw bytes for an already-loaded asset.
    pub fn get_raw(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        self.get_raw_bytes(id)
    }

    /// Unload a specific asset by handle, freeing its data.
    pub fn unload<T: ?Sized>(&mut self, handle: Handle<T>) {
        let index = handle.index() as usize;
        if index < self.slots.len() {
            let slot = &mut self.slots[index];
            if slot.generation == handle.generation() {
                let size = slot.size_bytes;
                if slot.data.take().is_some() {
                    self.memory.account_remove(size);
                    slot.pin_count = 0;
                    tracing::debug!("Unloaded slot {index}");
                }
            }
        }
    }

    /// Unload all assets.
    pub fn unload_all(&mut self) {
        for slot in &mut self.slots {
            if slot.data.take().is_some() {
                self.memory.account_remove(slot.size_bytes);
                slot.pin_count = 0;
            }
        }
        self.memory.current = 0;
        tracing::info!(
            "Unloaded all assets (memory freed: {})",
            self.memory.current
        );
    }

    /// Unload an asset by ID (convenience).
    pub fn unload_id(&mut self, id: AssetId) -> Result<(), RuntimeError> {
        let index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &mut self.slots[index as usize];
        let size = slot.size_bytes;
        if slot.data.take().is_some() {
            self.memory.account_remove(size);
            slot.pin_count = 0;
        }
        Ok(())
    }

    /// Pin an asset to prevent it from being evicted from memory.
    ///
    /// Increments the slot's pin count. As long as `pin_count > 0`,
    /// automatic eviction (LRU / FIFO) will never discard this asset's data.
    ///
    /// Returns the updated pin count on success.
    pub fn pin<T: ?Sized>(&mut self, handle: Handle<T>) -> Result<u32, RuntimeError> {
        let index = handle.index() as usize;
        if index >= self.slots.len() {
            return Err(RuntimeError::SlotEmpty {
                index: handle.index(),
            });
        }
        let slot = &mut self.slots[index];
        if slot.generation != handle.generation() {
            return Err(RuntimeError::GenerationMismatch {
                index: handle.index(),
                expected: handle.generation(),
                got: slot.generation,
            });
        }
        if slot.data.is_none() {
            return Err(RuntimeError::NotLoaded(slot.asset_id));
        }
        slot.pin_count = slot.pin_count.saturating_add(1);
        Ok(slot.pin_count)
    }

    /// Unpin an asset, allowing it to be evicted when memory is needed.
    ///
    /// Decrements the slot's pin count. If the pin count reaches 0, the
    /// asset becomes eligible for eviction.
    ///
    /// Returns the updated pin count on success, or `Err(RuntimeError::PinUnderflow)`
    /// if the asset was not pinned.
    pub fn unpin<T: ?Sized>(&mut self, handle: Handle<T>) -> Result<u32, RuntimeError> {
        let index = handle.index() as usize;
        if index >= self.slots.len() {
            return Err(RuntimeError::SlotEmpty {
                index: handle.index(),
            });
        }
        let slot = &mut self.slots[index];
        if slot.generation != handle.generation() {
            return Err(RuntimeError::GenerationMismatch {
                index: handle.index(),
                expected: handle.generation(),
                got: slot.generation,
            });
        }
        if slot.pin_count == 0 {
            return Err(RuntimeError::PinUnderflow(slot.asset_id));
        }
        slot.pin_count -= 1;
        Ok(slot.pin_count)
    }

    /// Check whether an asset handle is currently pinned.
    pub fn is_pinned<T: ?Sized>(&self, handle: Handle<T>) -> bool {
        self.pin_count(handle) > 0
    }

    /// Get the current pin count for an asset handle.
    pub fn pin_count<T: ?Sized>(&self, handle: Handle<T>) -> u32 {
        let index = handle.index() as usize;
        if index < self.slots.len() {
            let slot = &self.slots[index];
            if slot.generation == handle.generation() {
                return slot.pin_count;
            }
        }
        0
    }

    /// Pin an asset using a type-erased [`AnyHandle`].
    pub fn pin_any(&mut self, handle: AnyHandle) -> Result<u32, RuntimeError> {
        self.pin(Handle::<()>::from(handle))
    }

    /// Unpin an asset using a type-erased [`AnyHandle`].
    pub fn unpin_any(&mut self, handle: AnyHandle) -> Result<u32, RuntimeError> {
        self.unpin(Handle::<()>::from(handle))
    }

    /// Check whether a type-erased [`AnyHandle`] is pinned.
    pub fn is_any_pinned(&self, handle: AnyHandle) -> bool {
        self.is_pinned(Handle::<()>::from(handle))
    }

    /// Get the pin count for a type-erased [`AnyHandle`].
    pub fn any_pin_count(&self, handle: AnyHandle) -> u32 {
        self.pin_count(Handle::<()>::from(handle))
    }

    /// Pin an asset by `AssetId`.
    pub fn pin_id(&mut self, id: AssetId) -> Result<u32, RuntimeError> {
        let index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &mut self.slots[index as usize];
        if slot.data.is_none() {
            return Err(RuntimeError::NotLoaded(id));
        }
        slot.pin_count = slot.pin_count.saturating_add(1);
        Ok(slot.pin_count)
    }

    /// Unpin an asset by `AssetId`.
    pub fn unpin_id(&mut self, id: AssetId) -> Result<u32, RuntimeError> {
        let index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &mut self.slots[index as usize];
        if slot.pin_count == 0 {
            return Err(RuntimeError::PinUnderflow(id));
        }
        slot.pin_count -= 1;
        Ok(slot.pin_count)
    }

    /// Check whether an asset ID is currently pinned.
    pub fn is_id_pinned(&self, id: AssetId) -> bool {
        self.id_pin_count(id) > 0
    }

    /// Get the current pin count for an asset ID.
    pub fn id_pin_count(&self, id: AssetId) -> u32 {
        if let Some(index) = self.slot_index(id) {
            if let Some(slot) = self.slots.get(index as usize) {
                return slot.pin_count;
            }
        }
        0
    }

    /// Called by the [`HotReloadWatcher`] when a `.pak` file changes.
    ///
    /// Two things must happen together otherwise hot reload is fake:
    /// 1. **Swap reader** --- manager keeps file bytes opened (`PackageReader` holds
    ///    `Arc<Vec<u8>>`); if not replaced then subsequent `load` still reads old bytes;
    /// 2. **Only refresh resources still in memory** --- slots evicted should not be eagerly reloaded by hot reload,
    ///    they will read new data from new reader naturally on next `load`.
    ///
    /// Refresh is **in-place content replacement**: `generation` stays same, so [`Handle`] issued before hot reload remains valid
    /// (only incremented when **slot recycled to other resource**).
    ///
    /// Unknown path treated as new `.pak`, registered normally.
    pub fn on_pak_changed(&mut self, path: &Path) -> Result<(), RuntimeError> {
        let reader = PackageReader::open(path)?;

        let Some(index) = self
            .packages
            .iter()
            .position(|package| package.path.as_deref() == Some(path))
        else {
            tracing::info!("Hot-reload registered new package {}", path.display());
            self.register_package(LoadedPackage {
                path: Some(path.to_path_buf()),
                reader,
            });
            return Ok(());
        };

        self.packages[index].reader = reader;

        let ids: Vec<AssetId> = self.packages[index]
            .reader
            .records()
            .iter()
            .map(|record| AssetId::from_raw(record.id))
            .collect();

        let mut refreshed = 0usize;
        for asset_id in ids {
            let Some(&slot_index) = self.id_map.get(&asset_id) else {
                continue;
            };
            let slot_index = slot_index as usize;
            if self.slots[slot_index].data.is_none() {
                continue;
            }
            let Ok(Some(data)) = self.packages[index].reader.read_asset_data(asset_id) else {
                continue;
            };

            let new_size = data.len() as u64;
            let epoch = self.load_epoch;
            {
                let slot = &mut self.slots[slot_index];
                let old_size = slot.size_bytes;
                if old_size != new_size {
                    self.memory.account_remove(old_size);
                    self.memory.account_add(new_size);
                }
                slot.data = Some(data);
                slot.size_bytes = new_size;
                slot.last_access.set(Instant::now());
                slot.load_epoch = epoch;
            }
            self.load_epoch += 1;
            refreshed += 1;
            tracing::info!("Hot-reloaded asset {asset_id}");
        }

        tracing::info!(
            "Replaced package {} ({} loaded asset(s) refreshed)",
            path.display(),
            refreshed
        );
        Ok(())
    }

    /// Iterate all registered asset IDs and their types.
    pub fn assets(&self) -> impl Iterator<Item = (AssetId, AssetType)> + '_ {
        self.id_map.iter().map(|(id, &idx)| {
            let slot = &self.slots[idx as usize];
            (*id, slot.asset_type)
        })
    }

    /// Iterate all currently loaded (in-memory) asset IDs.
    pub fn loaded_assets(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.slots.iter().filter_map(|slot| {
            if slot.data.is_some() && slot.asset_id != AssetId::from_raw(0) {
                Some(slot.asset_id)
            } else {
                None
            }
        })
    }
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ResourceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceManager")
            .field("slots", &self.slots.len())
            .field("registered", &self.id_map.len())
            .field("packages", &self.packages.len())
            .field("memory_usage", &self.memory.current)
            .field("memory_budget", &self.memory.budget)
            .field("eviction_policy", &self.memory.policy)
            .finish()
    }
}

#[cfg(feature = "streaming")]
enum StreamIter<'a> {
    Slice(SliceStreamIter<'a>),
    Compressed(CompressedStreamIter<'a>),
}

#[cfg(feature = "streaming")]
struct SliceStreamIter<'a> {
    /// Borrowed for in-memory packages, owned when the bytes came from disk.
    data: Cow<'a, [u8]>,
    pos: usize,
    chunk_size: usize,
}

#[cfg(feature = "streaming")]
impl<'a> Iterator for SliceStreamIter<'a> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.data.len() {
            return None;
        }
        let end = (self.pos + self.chunk_size).min(self.data.len());
        let chunk = self.data[self.pos..end].to_vec();
        self.pos = end;
        Some(chunk)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.data.len() - self.pos;
        let chunks = remaining.div_ceil(self.chunk_size.max(1));
        (chunks, Some(chunks))
    }
}

#[cfg(feature = "streaming")]
struct CompressedStreamIter<'a> {
    /// Boxed so the decoder can read either a borrowed slice or an owned
    /// buffer read from disk, without making the struct self-referential.
    decoder: zstd::Decoder<'static, std::io::BufReader<Box<dyn std::io::Read + 'a>>>,
    chunk_size: usize,
    total_size: usize,
    read_so_far: usize,
}

#[cfg(feature = "streaming")]
impl<'a> Iterator for CompressedStreamIter<'a> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.read_so_far >= self.total_size {
            return None;
        }
        let mut buf = vec![0u8; self.chunk_size];
        let mut total_read = 0;
        while total_read < self.chunk_size {
            match std::io::Read::read(&mut self.decoder, &mut buf[total_read..]) {
                Ok(0) => break,
                Ok(n) => total_read += n,
                Err(_) => return None,
            }
        }
        if total_read == 0 {
            return None;
        }
        buf.truncate(total_read);
        self.read_so_far += total_read;
        Some(buf)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.total_size.saturating_sub(self.read_so_far);
        let chunks = remaining.div_ceil(self.chunk_size.max(1));
        (chunks, Some(chunks))
    }
}

#[cfg(feature = "streaming")]
impl<'a> Iterator for StreamIter<'a> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Slice(it) => it.next(),
            Self::Compressed(it) => it.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Slice(it) => it.size_hint(),
            Self::Compressed(it) => it.size_hint(),
        }
    }
}

/// Trait that asset types must implement to be loadable through the
/// [`ResourceManager`].
pub trait Asset: Sized + Send + 'static {
    /// The expected [`AssetType`] for this type.
    fn asset_type() -> AssetType;

    /// Deserialize from raw bytes.
    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError>;

    /// Serialize back to bytes.
    fn into_bytes(self) -> Vec<u8>;
}

/// Simple blob asset — wraps raw bytes.
impl Asset for Vec<u8> {
    fn asset_type() -> AssetType {
        AssetType::Binary
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        Ok(data.to_vec())
    }

    fn into_bytes(self) -> Vec<u8> {
        self
    }
}

#[cfg(test)]
mod tests;
