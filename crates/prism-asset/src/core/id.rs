use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Global atomic counter used by AssetId::generate()
// ---------------------------------------------------------------------------

/// Process-wide serial counter used by [`AssetId::generate`].
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// AssetId — globally unique u64-based identifier
// ---------------------------------------------------------------------------

/// A globally unique 64-bit asset identifier.
///
/// The high 32 bits encode a **generation** (monotonically increasing epoch),
/// the low 32 bits encode a **serial** within that generation.
///
/// `tombstone` values use generation `u32::MAX` — they compare higher than
/// any live ID and can be used as deletion sentinels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct AssetId(pub u64);

impl AssetId {
    /// Serial mask: the low 32 bits.
    const SERIAL_MASK: u64 = 0x0000_0000_FFFF_FFFF;
    /// Shift for the generation field.
    const GENERATION_SHIFT: u64 = 32;

    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new `AssetId` from its raw u64 representation.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Extract the raw u64. Useful for storage / checksums.
    pub const fn into_raw(self) -> u64 {
        self.0
    }

    /// Generate a fresh process-local ID using an atomic counter.
    ///
    /// Runtime-generated IDs use **generation 0**, which is reserved away from
    /// the persisted cooker/database namespace (generation >= 1) so that
    /// `AssetId::generate()` can never collide with `AssetIdGenerator::new(1, ..)`.
    ///
    /// This is **not** persisted across restarts. Editor code should
    /// call [`AssetIdGenerator::next`] if it has access to the database
    /// sequence.
    pub fn generate() -> Self {
        let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
        assert!(
            serial <= Self::SERIAL_MASK,
            "runtime AssetId serial space exhausted"
        );
        Self(serial)
    }

    /// Build a tombstone sentinel for the given serial.
    /// Tombstones are IDs with generation = `u32::MAX` — they sort after
    /// every live ID and can never collide with a future `generate()` call.
    pub fn tombstone(serial: u64) -> Self {
        let serial = serial & Self::SERIAL_MASK;
        Self((u64::from(u32::MAX) << Self::GENERATION_SHIFT) | serial)
    }

    /// Returns `true` when this ID is a tombstone (deleted marker).
    pub fn is_tombstone(self) -> bool {
        self.generation() == u32::MAX
    }

    /// Stable ID derived from asset-relative path (for build-time packing, #530).
    ///
    /// `pack_directory` uses it to assign ID to each file in package; texture/prefab baked at cook time
    /// types that bake dependency ID into data blocks (`RMAT` stores `AssetId`) therefore can
    /// compute pack-consistent IDs before packing — sequential allocation can't do this: adding
    /// a file to the directory shifts all subsequent IDs, texture IDs baked into materials all point wrong.
    ///
    /// Path is normalized on `/` (\` -> `/`), so Windows and POSIX builds produce the same
    /// ID; generation is fixed to 1 (cooker/database namespace), serial takes FNV-1a
    /// low 32 bits avoiding 0. 32-bit space still has tiny collision probability across
    /// different paths; packer detects and errors rather than silently overwriting.
    pub fn from_path(path: &str) -> Self {
        Self::from_raw((1u64 << Self::GENERATION_SHIFT) | u64::from(path_serial(path).max(1)))
    }

    /// Extract the generation component.
    pub fn generation(self) -> u32 {
        (self.0 >> Self::GENERATION_SHIFT) as u32
    }

    /// Extract the serial component.
    pub fn serial(self) -> u32 {
        (self.0 & Self::SERIAL_MASK) as u32
    }
}

// ---------------------------------------------------------------------------
// Display / Debug
// ---------------------------------------------------------------------------

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId({:016x})", self.0)
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AssetId({:016x} gen={} serial={})",
            self.0,
            self.generation(),
            self.serial()
        )
    }
}

// ---------------------------------------------------------------------------
// Serde
// ---------------------------------------------------------------------------

impl serde::Serialize for AssetId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for AssetId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64::deserialize(d).map(Self)
    }
}

// ---------------------------------------------------------------------------
// Persistent ID generator (editor)
// ---------------------------------------------------------------------------

/// A persisted asset-ID generator that tracks the next serial in a file.
pub struct AssetIdGenerator {
    next_serial: u64,
    generation: u32,
}

impl AssetIdGenerator {
    /// Create a fresh generator. The initial serial is loaded from
    /// `current_max` (0 = start from serial 1).
    pub fn new(generation: u32, current_max: u64) -> Self {
        Self {
            next_serial: current_max + 1,
            generation,
        }
    }

    /// Allocate the next asset ID (monotonically increasing).
    ///
    /// Panics after 2^32 allocations instead of silently truncating the serial
    /// and reusing IDs.
    #[expect(
        clippy::should_implement_trait,
        reason = "`from_raw` is the deliberate deserialization entry point"
    )]
    pub fn next(&mut self) -> AssetId {
        let serial = self.next_serial;
        assert!(
            serial <= AssetId::SERIAL_MASK,
            "AssetIdGenerator serial space exhausted for generation {}",
            self.generation
        );
        self.next_serial += 1;
        AssetId((u64::from(self.generation) << AssetId::GENERATION_SHIFT) | serial)
    }

    /// Current serial value (next to be assigned).
    pub fn current_serial(&self) -> u64 {
        self.next_serial
    }

    /// Generation epoch.
    pub fn generation(&self) -> u32 {
        self.generation
    }
}

/// FNV-1a 32-bit hash of `path` (path normalized on `/` first).
///
/// FNV-1a is chosen because it is short, dependency-free, and cross-platform consistent;
/// the purpose here is just "same path always gets same ID", not cryptographic strength.
fn path_serial(path: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in path.replace('\\', "/").as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
