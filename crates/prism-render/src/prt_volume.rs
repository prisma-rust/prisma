//! `.prtbin` payload: PRT64 cluster volume encoding and decoding.
//!
//! Contract: `docs/prt-gi-64tree-design.md` §2. The file is a 64-byte header
//! followed by a breadth-first node table and a cluster-major coefficient body.
//! Nothing here touches Vulkan; this module is the single source of truth for the
//! on-disk layout, and the offline baker and the runtime loader both go through it.
//!
//! ```text
//! header (64 B) | node table (node_count * 16 B) | cluster body (leaf_count * 384 B)
//! ```
//!
//! Each cluster stores 64 real-SH coefficients as RGB triplets. The body is `f16` on
//! disk because the size budget is defined in terms of that encoding (§2.4); decoding
//! promotes to `f32` immediately, and every consumer computes in `f32`.

use std::fmt;
use std::io;
use std::path::Path;

/// Magic bytes identifying a PrismaRev PRT volume.
pub const MAGIC: &[u8; 4] = b"PRTB";
/// On-disk format version.
pub const VERSION: u16 = 1;
/// Byte size of the fixed header.
pub const HEADER_SIZE: usize = 64;
/// Byte size of one node-table entry.
pub const NODE_SIZE: usize = 16;
/// Byte size of one cluster record at the f16 encoding.
pub const CLUSTER_SIZE: usize = 384;
/// Real-SH coefficient count of the 7th-order basis: `(7 + 1)^2 = 64`.
pub const SH_COEFF_COUNT: usize = 64;
/// Largest leaf count whose body still fits the 3.5 MB budget.
pub const LEAF_COUNT_MAX: u32 = 8600;
/// Branching factor of the spatial tree: `4 * 4 * 4` children per node.
pub const BRANCHING: u32 = 64;

/// Payload carries a trailing one-byte `valid` flag per cluster.
pub const FLAG_VALID: u16 = 1 << 1;
/// The v1 payload encoding. Always set: the cluster body is f16.
pub const FLAG_PAYLOAD_F16: u16 = 1 << 0;

/// Sentinel in `PrtNode::first_child` marking a leaf.
pub const NO_CHILD: u32 = u32::MAX;
/// Sentinel in `PrtNode::leaf_index` marking an interior node.
pub const NO_LEAF: u32 = u32::MAX;

/// One entry of the breadth-first node table.
///
/// `first_child` is monotonic because the table is breadth-first, so a streaming
/// loader can claim a subtree with one linear scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrtNode {
    /// Index of the first child node, or [NO_CHILD] for a leaf.
    pub first_child: u32,
    /// Index into the cluster list, or [NO_LEAF] for an interior node.
    pub leaf_index: u32,
    /// Bit `i` is set when child `i` exists. Stored as two `u32` words on the GPU.
    pub child_mask: u64,
}

impl PrtNode {
    /// Whether this node holds a cluster instead of children.
    pub fn is_leaf(&self) -> bool {
        self.first_child == NO_CHILD
    }

    /// Whether child `i` exists.
    pub fn has_child(&self, i: u32) -> bool {
        i < BRANCHING && (self.child_mask >> i) & 1 == 1
    }

    /// Number of present children.
    pub fn child_count(&self) -> u32 {
        (0..BRANCHING).filter(|&i| self.has_child(i)).count() as u32
    }
}

/// One cluster's 64 real-SH coefficients, RGB per coefficient in coefficient order.
pub type PrtCluster = [[f32; 3]; SH_COEFF_COUNT];

/// A decoded PRT64 cluster volume.
///
/// Coefficient `c` of cluster `k` lives at `clusters[k][c]`; the ordering matches
/// `crates/prism-render/src/gi.rs`, extended from order 2 to order 7.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrtVolume {
    /// World position of the root node's minimum corner.
    pub world_origin: [f32; 3],
    /// Edge length in metres of the root node's cube.
    pub world_extent: f32,
    /// Leaf edge is `world_extent / 4^leaf_grid`.
    pub leaf_grid: u32,
    /// Spatial cells per cluster; `1` means one cell per leaf.
    pub cluster_cells: u32,
    /// First eight bytes of the source package SHA-256, for mismatch rejection.
    pub built_from_sha: [u8; 8],
    /// Breadth-first node table.
    pub nodes: Vec<PrtNode>,
    /// Cluster-major coefficient body.
    pub clusters: Vec<PrtCluster>,
    /// Per-cluster validity flags, present only when [FLAG_VALID] is set.
    pub cluster_valid: Option<Vec<bool>>,
}

/// Decode or encode failure.
#[derive(Debug)]
pub enum PrtError {
    /// Header shorter than [HEADER_SIZE].
    TruncatedHeader(usize),
    /// Magic bytes did not match [MAGIC].
    BadMagic([u8; 4]),
    /// Version is not [VERSION].
    UnsupportedVersion(u16),
    /// `flags` carried a bit this version does not define.
    UnsupportedFlags(u16),
    /// `sh_band` was not 7.
    BadShBand(u32),
    /// `leaf_count` exceeded [LEAF_COUNT_MAX].
    TooManyLeaves(u32),
    /// `payload_bytes` disagreed with the counts in the header.
    PayloadSizeMismatch {
        /// Value read from the header.
        declared: u32,
        /// Value implied by the leaf count and encoding.
        expected: u32,
    },
    /// `node_count` was zero.
    NoNodes,
    /// File ended before the declared body.
    Truncated {
        /// Bytes the header promises.
        need: usize,
        /// Bytes actually present.
        have: usize,
    },
    /// Cluster count disagreed with the number of leaves in the node table.
    LeafCountMismatch {
        /// Leaves found in the node table.
        leaves: u32,
        /// Clusters decoded from the body.
        clusters: usize,
    },
    /// `valid` flag count disagreed with the cluster count.
    ValidFlagMismatch {
        /// Clusters decoded from the body.
        clusters: usize,
        /// Validity flags decoded from the body.
        flags: usize,
    },
    /// A coefficient was not finite.
    NonFiniteCoefficient {
        /// Cluster index of the offending coefficient.
        cluster: usize,
        /// Coefficient index inside the cluster.
        coeff: usize,
    },
    /// Filesystem error.
    Io(io::Error),
}

impl fmt::Display for PrtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrtError::TruncatedHeader(n) => {
                write!(f, "file too small: {n} bytes, need at least {HEADER_SIZE}")
            }
            PrtError::BadMagic(m) => write!(f, "invalid magic {m:?} (expected PRTB)"),
            PrtError::UnsupportedVersion(v) => {
                write!(f, "unsupported version {v} (expected {VERSION})")
            }
            PrtError::UnsupportedFlags(v) => write!(f, "unsupported header flags {v:#06x}"),
            PrtError::BadShBand(v) => {
                write!(f, "sh_band {v} is not 7 ({SH_COEFF_COUNT} coefficients)")
            }
            PrtError::TooManyLeaves(n) => write!(
                f,
                "leaf_count {n} exceeds the {LEAF_COUNT_MAX} budget (3.5 MB at {CLUSTER_SIZE} B per cluster)"
            ),
            PrtError::PayloadSizeMismatch { declared, expected } => write!(
                f,
                "payload_bytes {declared} does not match the {expected} bytes implied by the header"
            ),
            PrtError::NoNodes => write!(f, "node_count must be at least 1"),
            PrtError::Truncated { need, have } => {
                write!(f, "body truncated: {have} bytes available, need {need}")
            }
            PrtError::LeafCountMismatch { leaves, clusters } => write!(
                f,
                "node table declares {leaves} leaves but {clusters} clusters were decoded"
            ),
            PrtError::ValidFlagMismatch { clusters, flags } => write!(
                f,
                "{clusters} clusters but {flags} validity flags"
            ),
            PrtError::NonFiniteCoefficient { cluster, coeff } => {
                write!(f, "cluster {cluster} coefficient {coeff} is not finite")
            }
            PrtError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PrtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PrtError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for PrtError {
    fn from(e: io::Error) -> Self {
        PrtError::Io(e)
    }
}

impl PrtVolume {
    /// Number of clusters in the body.
    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }

    /// Number of leaves declared by the node table.
    pub fn leaf_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_leaf()).count()
    }

    /// Bytes the body occupies at the f16 encoding.
    pub fn payload_bytes(&self) -> usize {
        let valid = if self.cluster_valid.is_some() {
            self.clusters.len()
        } else {
            0
        };
        self.clusters.len() * CLUSTER_SIZE + valid
    }

    /// Check structural invariants that the wire format alone cannot express.
    pub fn validate(&self) -> Result<(), PrtError> {
        if self.nodes.is_empty() {
            return Err(PrtError::NoNodes);
        }
        let leaves = self.leaf_count();
        if leaves != self.clusters.len() {
            return Err(PrtError::LeafCountMismatch {
                leaves: leaves as u32,
                clusters: self.clusters.len(),
            });
        }
        if leaves > LEAF_COUNT_MAX as usize {
            return Err(PrtError::TooManyLeaves(leaves as u32));
        }
        if let Some(valid) = &self.cluster_valid {
            if valid.len() != self.clusters.len() {
                return Err(PrtError::ValidFlagMismatch {
                    clusters: self.clusters.len(),
                    flags: valid.len(),
                });
            }
        }
        for (cluster, coeffs) in self.clusters.iter().enumerate() {
            for (coeff, rgb) in coeffs.iter().enumerate() {
                if !rgb.iter().all(|c| c.is_finite()) {
                    return Err(PrtError::NonFiniteCoefficient { cluster, coeff });
                }
            }
        }
        Ok(())
    }

    /// Serialize to the `.prtbin` wire format.
    pub fn to_bytes(&self) -> Result<Vec<u8>, PrtError> {
        self.validate()?;

        let flags = FLAG_PAYLOAD_F16
            | if self.cluster_valid.is_some() {
                FLAG_VALID
            } else {
                0
            };
        let payload_bytes = self.payload_bytes();
        let total = HEADER_SIZE + self.nodes.len() * NODE_SIZE + payload_bytes;
        let mut buf = Vec::with_capacity(total);

        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&VERSION.to_le_bytes());
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&7u32.to_le_bytes());
        buf.extend_from_slice(&(self.clusters.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.cluster_cells.to_le_bytes());
        for axis in self.world_origin {
            buf.extend_from_slice(&axis.to_le_bytes());
        }
        buf.extend_from_slice(&self.world_extent.to_le_bytes());
        buf.extend_from_slice(&self.leaf_grid.to_le_bytes());
        buf.extend_from_slice(&(payload_bytes as u32).to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        buf.extend_from_slice(&self.built_from_sha);

        for node in &self.nodes {
            buf.extend_from_slice(&node.first_child.to_le_bytes());
            buf.extend_from_slice(&node.leaf_index.to_le_bytes());
            buf.extend_from_slice(&node.child_mask.to_le_bytes());
        }

        for cluster in &self.clusters {
            for rgb in cluster {
                for channel in rgb {
                    buf.extend_from_slice(&f32_to_f16(*channel).to_le_bytes());
                }
            }
        }

        if let Some(valid) = &self.cluster_valid {
            buf.extend(valid.iter().map(|&b| u8::from(b)));
        }

        debug_assert_eq!(buf.len(), total);
        Ok(buf)
    }

    /// Decode the `.prtbin` wire format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PrtError> {
        if bytes.len() < HEADER_SIZE {
            return Err(PrtError::TruncatedHeader(bytes.len()));
        }
        let magic: [u8; 4] = bytes[0..4].try_into().expect("4-byte slice");
        if &magic != MAGIC {
            return Err(PrtError::BadMagic(magic));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != VERSION {
            return Err(PrtError::UnsupportedVersion(version));
        }
        let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
        if flags & !(FLAG_PAYLOAD_F16 | FLAG_VALID) != 0 || flags & FLAG_PAYLOAD_F16 == 0 {
            return Err(PrtError::UnsupportedFlags(flags));
        }
        let sh_band = read_u32(&bytes[8..12]);
        if sh_band != 7 {
            return Err(PrtError::BadShBand(sh_band));
        }
        let leaf_count = read_u32(&bytes[12..16]);
        if leaf_count > LEAF_COUNT_MAX {
            return Err(PrtError::TooManyLeaves(leaf_count));
        }
        let node_count = read_u32(&bytes[16..20]);
        if node_count == 0 {
            return Err(PrtError::NoNodes);
        }
        let cluster_cells = read_u32(&bytes[20..24]);
        let world_origin = [
            read_f32(&bytes[24..28]),
            read_f32(&bytes[28..32]),
            read_f32(&bytes[32..36]),
        ];
        let world_extent = read_f32(&bytes[36..40]);
        let leaf_grid = read_u32(&bytes[40..44]);
        let payload_bytes = read_u32(&bytes[44..48]);
        let built_from_sha: [u8; 8] = bytes[56..64].try_into().expect("8-byte slice");

        let has_valid = flags & FLAG_VALID != 0;
        let body_per_cluster = CLUSTER_SIZE + usize::from(has_valid);
        let expected_payload = leaf_count as usize * body_per_cluster;
        if payload_bytes as usize != expected_payload {
            return Err(PrtError::PayloadSizeMismatch {
                declared: payload_bytes,
                expected: expected_payload as u32,
            });
        }

        let node_bytes = node_count as usize * NODE_SIZE;
        let need = HEADER_SIZE + node_bytes + expected_payload;
        if bytes.len() < need {
            return Err(PrtError::Truncated {
                need,
                have: bytes.len(),
            });
        }

        let mut nodes = Vec::with_capacity(node_count as usize);
        for i in 0..node_count as usize {
            let base = HEADER_SIZE + i * NODE_SIZE;
            nodes.push(PrtNode {
                first_child: read_u32(&bytes[base..base + 4]),
                leaf_index: read_u32(&bytes[base + 4..base + 8]),
                child_mask: u64::from_le_bytes(
                    bytes[base + 8..base + 16].try_into().expect("8-byte slice"),
                ),
            });
        }

        let body_base = HEADER_SIZE + node_bytes;
        let mut clusters = Vec::with_capacity(leaf_count as usize);
        for k in 0..leaf_count as usize {
            let cluster_base = body_base + k * body_per_cluster;
            let mut cluster = [[0.0f32; 3]; SH_COEFF_COUNT];
            for (c, slot) in cluster.iter_mut().enumerate() {
                let coeff_base = cluster_base + c * 3 * 2;
                for (ch, value) in slot.iter_mut().enumerate() {
                    let at = coeff_base + ch * 2;
                    let bits = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    *value = f16_to_f32(bits);
                }
            }
            clusters.push(cluster);
        }

        let cluster_valid = if has_valid {
            let valid_base = body_base + leaf_count as usize * CLUSTER_SIZE;
            Some(
                (0..leaf_count as usize)
                    .map(|k| bytes[valid_base + k] != 0)
                    .collect(),
            )
        } else {
            None
        };

        Ok(PrtVolume {
            world_origin,
            world_extent,
            leaf_grid,
            cluster_cells,
            built_from_sha,
            nodes,
            clusters,
            cluster_valid,
        })
    }

    /// Read a `.prtbin` file.
    pub fn load(path: &Path) -> Result<Self, PrtError> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Write a `.prtbin` file.
    pub fn save(&self, path: &Path) -> Result<(), PrtError> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }
}

/// Convert a finite `f32` to the nearest IEEE-754 binary16 bit pattern.
///
/// Round-to-nearest-even, matching the hardware conversion the GPU upload expects.
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x007F_FFFF;

    if exp == 0xFF {
        // Infinity keeps its exponent; NaN keeps enough mantissa to stay quiet.
        let payload = if mant == 0 { 0 } else { 0x0200 };
        return sign | 0x7C00 | payload;
    }

    let unbiased = exp - 127;
    if unbiased > 15 {
        return sign | 0x7C00;
    }
    if unbiased < -14 {
        // Subnormal half: the value is M * 2^-24 with 24-bit M, so shift the
        // implicit-bit mantissa right by -(unbiased + 1) to land on M.
        let drop = (-unbiased - 1) as u32;
        if drop > 25 {
            // Below half the smallest subnormal: rounds to zero.
            return sign;
        }
        let mant = mant | 0x0080_0000;
        let mut half_mant = (mant >> drop) as u16;
        let rem = mant & ((1u32 << drop) - 1);
        let halfway = 1u32 << (drop - 1);
        if rem > halfway || (rem == halfway && half_mant & 1 == 1) {
            half_mant += 1;
        }
        // 0x0400 is the smallest normal half, which is the correct encoding when
        // the subnormal mantissa rounds all the way up.
        return sign | half_mant;
    }

    let mut half_exp = (unbiased + 15) as u16;
    let mut half_mant = (mant >> 13) as u16;
    let rem = mant & 0x1FFF;
    if rem > 0x1000 || (rem == 0x1000 && half_mant & 1 == 1) {
        half_mant += 1;
        if half_mant == 0x0400 {
            half_mant = 0;
            half_exp += 1;
            if half_exp >= 0x1F {
                return sign | 0x7C00;
            }
        }
    }
    sign | (half_exp << 10) | half_mant
}

/// Convert an IEEE-754 binary16 bit pattern to `f32`.
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1F) as u32;
    let mant = (bits & 0x03FF) as u32;

    let out = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            // Normalize the subnormal into an f32 normal.
            let mut e: u32 = 113; // 127 - 14
            let mut m = mant;
            while m & 0x0400 == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x03FF;
            sign | (e << 23) | (m << 13)
        }
    } else if exp == 0x1F {
        sign | 0x7F80_0000 | (mant << 13)
    } else {
        sign | ((exp + 112) << 23) | (mant << 13)
    };
    f32::from_bits(out)
}

fn read_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn read_f32(b: &[u8]) -> f32 {
    f32::from_bits(read_u32(b))
}
