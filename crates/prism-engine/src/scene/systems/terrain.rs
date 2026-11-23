//! Heightfield terrain runtime (#517 Phase 1).
//!
//! [`Terrain`] in scene is only data: here read it into [`Heightfield`], generate a
//! `MeshUploadInput` (positions / normals / uv0 / indices), register into
//! [`ProceduralMeshes`], then point same entity's `MeshRenderer.mesh_path` to this virtual path.
//! Asset parser seeing `__proc/` prefix takes mesh directly from [`ProceduralMeshes`], no cook needed.
//!
//! Query interface: [`Heightfield::sample_height`] / [`Heightfield::sample_normal`] are pure functions,
//! [`terrain_height`] / [`terrain_sample`] find covering terrain at world point by `TerrainData`,
//! [`terrain_raycast`] uses same sampling for parsed raycast query (gameplay landing / picking).
//!
//! Multi-chunk + LOD + vegetation implemented (#517 Phase 2): when `Terrain.chunks > 1` each chunk is
//! independent child entity / independent mesh, render and shadow can cull per chunk; `Terrain.lod_distances`
//! controls 1–3 LOD levels, whole terrain switches level uniformly by nearest camera (avoid cross-chunk cracks).
//! `Terrain.vegetation_*` scatters shared-mesh instances deterministically by height / slope, handed to renderer
//! static instancing batching. [`Heightfield::sculpt`], `prism-build-pipeline sculpt-heightmap`
//! and editor Scene viewport Sculpt mode provide raise / lower / smooth / flatten brushes;
//! `Terrain.splat` provides 4-layer procedural color bands by height / slope (vertex colors);
//! `Terrain.splat_materials` provides up to 4 layers of texture splat (albedo / normal /
//! metallic-roughness blended by vertex weights). rapier terrain collision body maintained by `prism-app`'s
//! physics thread (see `physics_runner`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::scene::components::{
    Camera, Child, LocalToWorld, LocalTransform, MaterialRef, MeshRef, MeshRenderer, Terrain,
};
use crate::scene::helpers::HierarchyHelper;
use prism_render::managers::mesh_manager::MeshHandle;
use prism_render::managers::mesh_manager::MeshUploadInput;
use prism_scene::components::SceneAssetId;
use prism_scene::{CullMode, ShadowCastingMode};

/// Prefix for in-process generated mesh paths; not cooked for them, not in `.pak`.
pub const PROCEDURAL_PREFIX: &str = "__proc/";

/// Parsed heightfield: row-major, `cells × cells` heights.
///
/// Value used is unit from `.raw` — heightmap CLI writes **meters** (absolute elevation, can be negative),
/// while hand-authored / normalized [0,1] heightfield scaled to meters via `height_scale`.
#[derive(Debug, Clone, PartialEq)]
pub struct Heightfield {
    cells: usize,
    heights: Vec<f32>,
}

/// Use of vertex color: none / procedural color band / texture layer 4-layer weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VertexSplat {
    None,
    Palette,
    Weights,
}

/// Heightfield brush mode (#517 Phase 2 sculpt foundation layer).
///
/// The enum lives in `prism-scene` so the pipeline CLI can sculpt a raw
/// heightmap without linking the engine; this re-export keeps the scene-side
/// path unchanged.
pub use prism_scene::terrain::SculptMode;

impl Heightfield {
    /// Parse from `.raw` (f32 little-endian, row-major).
    ///
    /// Length must be exactly `cells * cells * 4`; shorter or longer returns `None` (`resolution`
    /// and file mismatch prefer reporting error over drawing a wrong terrain). Height values not clamped to range —
    /// heightmap CLI writes meters (with negative values), normalized heightfield scaled by `height_scale`.
    pub fn from_raw(bytes: &[u8], cells: usize) -> Option<Self> {
        prism_scene::terrain::decode_heights(bytes, cells).map(|heights| Self { cells, heights })
    }

    /// Infer resolution from `.raw`: sample count must be perfect square (heightmap CLI default 1024×1024).
    pub fn from_raw_auto(bytes: &[u8]) -> Option<Self> {
        prism_scene::terrain::decode_heights_auto(bytes)
            .map(|(cells, heights)| Self { cells, heights })
    }

    /// All-zero heightfield (placeholder when no heightmap, still a flat visible surface).
    pub fn flat(cells: usize) -> Self {
        let cells = cells.max(2);
        Self {
            cells,
            heights: vec![0.0; cells * cells],
        }
    }

    pub fn cells(&self) -> usize {
        self.cells
    }

    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// Height at mesh coordinates: `u` / `v` are `[0, 1]` cell coords (out of bounds clamped to boundary).
    pub fn sample_height(&self, u: f32, v: f32) -> f32 {
        prism_scene::terrain::sample_height(self.cells, &self.heights, u, v)
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.heights[y * self.cells + x]
    }

    /// Normal at mesh coordinates (central difference, Y already normalized).
    pub fn sample_normal(&self, u: f32, v: f32, height_scale: f32) -> [f32; 3] {
        let step = 1.0 / (self.cells - 1) as f32;
        let left = self.sample_height(u - step, v) * height_scale;
        let right = self.sample_height(u + step, v) * height_scale;
        let down = self.sample_height(u, v - step) * height_scale;
        let up = self.sample_height(u, v + step) * height_scale;
        // Mesh is equidistant samples over world size, so gradient only needs scaling by cell distance.
        let dx = (right - left) / (2.0 * step).max(1e-6);
        let dz = (up - down) / (2.0 * step).max(1e-6);
        let normal = glam::Vec3::new(-dx, 1.0, -dz).normalize_or_zero();
        if normal == glam::Vec3::ZERO {
            [0.0, 1.0, 0.0]
        } else {
            [normal.x, normal.y, normal.z]
        }
    }

    /// Generate mesh for whole terrain (single chunk): spread over `size`, `y = height * height_scale`.
    pub fn build_mesh(&self, size: [f32; 2], height_scale: f32) -> MeshUploadInput {
        self.build_mesh_with_splat(size, height_scale, false)
    }

    /// Single mesh + optional procedural color band splat (#517).
    pub fn build_mesh_with_splat(
        &self,
        size: [f32; 2],
        height_scale: f32,
        splat: bool,
    ) -> MeshUploadInput {
        self.build_chunk_mesh_lod_with_splat(
            size,
            height_scale,
            0,
            self.cells - 1,
            0,
            self.cells - 1,
            1,
            splat,
        )
    }

    /// Generate mesh for `[x0, x1] × [z0, z1]` cell range (#517 Phase 2).
    ///
    /// Vertex coords relative to **this chunk's center**, for attaching chunk entity under parent terrain; UV still uses whole
    /// terrain's `[0, 1]`, so adjacent chunks' materials won't reset at seams. Range auto-sorted,
    /// clamped to `0..=cells-1`, at least one quad kept.
    pub fn build_chunk_mesh(
        &self,
        size: [f32; 2],
        height_scale: f32,
        x0: usize,
        x1: usize,
        z0: usize,
        z1: usize,
    ) -> MeshUploadInput {
        self.build_chunk_mesh_lod(size, height_scale, x0, x1, z0, z1, 1)
    }

    /// Chunk mesh with step (#517 LOD): `step = 1/2/4` for LOD0/1/2.
    ///
    /// Both end cells always kept, so each level's boundary matches adjacent chunks / finer level; within level
    /// only interior sample rows reduced. Whole terrain selecting level uniformly avoids cross-chunk cracks.
    #[expect(
        clippy::too_many_arguments,
        reason = "chunk bounds and LOD step are one flat rectangle plus a stride"
    )]
    pub fn build_chunk_mesh_lod(
        &self,
        size: [f32; 2],
        height_scale: f32,
        x0: usize,
        x1: usize,
        z0: usize,
        z1: usize,
        step: usize,
    ) -> MeshUploadInput {
        self.build_chunk_mesh_lod_with_splat(size, height_scale, x0, x1, z0, z1, step, false)
    }

    /// Chunk mesh with step + optional procedural color band splat.
    #[expect(
        clippy::too_many_arguments,
        reason = "chunk bounds and LOD step are one flat rectangle plus a stride"
    )]
    pub fn build_chunk_mesh_lod_with_splat(
        &self,
        size: [f32; 2],
        height_scale: f32,
        x0: usize,
        x1: usize,
        z0: usize,
        z1: usize,
        step: usize,
        splat: bool,
    ) -> MeshUploadInput {
        self.build_chunk_mesh_lod_mode(
            size,
            height_scale,
            x0,
            x1,
            z0,
            z1,
            step,
            if splat {
                VertexSplat::Palette
            } else {
                VertexSplat::None
            },
        )
    }

    /// 4-layer weights of texture layer splat (written to vertex color R/G/B, 4th layer implicit).
    #[expect(
        clippy::too_many_arguments,
        reason = "chunk bounds and LOD step are one flat rectangle plus a stride"
    )]
    pub fn build_chunk_mesh_lod_with_weights(
        &self,
        size: [f32; 2],
        height_scale: f32,
        x0: usize,
        x1: usize,
        z0: usize,
        z1: usize,
        step: usize,
    ) -> MeshUploadInput {
        self.build_chunk_mesh_lod_mode(
            size,
            height_scale,
            x0,
            x1,
            z0,
            z1,
            step,
            VertexSplat::Weights,
        )
    }

    /// Single mesh + texture layer weights (for no-chunk legacy path).
    pub fn build_mesh_with_weights(&self, size: [f32; 2], height_scale: f32) -> MeshUploadInput {
        self.build_chunk_mesh_lod_mode(
            size,
            height_scale,
            0,
            self.cells - 1,
            0,
            self.cells - 1,
            1,
            VertexSplat::Weights,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "chunk bounds, LOD step and splat mode are one flat rectangle plus selectors"
    )]
    fn build_chunk_mesh_lod_mode(
        &self,
        size: [f32; 2],
        height_scale: f32,
        x0: usize,
        x1: usize,
        z0: usize,
        z1: usize,
        step: usize,
        splat_mode: VertexSplat,
    ) -> MeshUploadInput {
        let cells = self.cells;
        let last = cells - 1;
        let (x0, x1) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        let (z0, z1) = if z0 <= z1 { (z0, z1) } else { (z1, z0) };
        let x0 = x0.min(last.saturating_sub(1));
        let x1 = x1.min(last).max(x0 + 1);
        let z0 = z0.min(last.saturating_sub(1));
        let z1 = z1.min(last).max(z0 + 1);

        let [size_x, size_z] = [size[0].max(0.001), size[1].max(0.001)];
        let last_f = last as f32;
        // Chunk center position in whole terrain local space; vertices subtract it.
        let center_x = ((x0 + x1) as f32 * 0.5 / last_f - 0.5) * size_x;
        let center_z = ((z0 + z1) as f32 * 0.5 / last_f - 0.5) * size_z;
        let xs = sample_indices(x0, x1, step);
        let zs = sample_indices(z0, z1, step);
        let columns = xs.len();
        let rows = zs.len();

        let splat = splat_mode != VertexSplat::None;
        let (min_height, max_height) = if splat {
            self.heights
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), height| {
                    (min.min(*height), max.max(*height))
                })
        } else {
            (0.0, 0.0)
        };
        let mut positions = Vec::with_capacity(columns * rows);
        let mut normals = Vec::with_capacity(columns * rows);
        let mut uvs = Vec::with_capacity(columns * rows);
        let mut colors = Vec::with_capacity(if splat { columns * rows } else { 0 });
        for &z in &zs {
            for &x in &xs {
                let (u, v) = (x as f32 / last_f, z as f32 / last_f);
                positions.push([
                    (u - 0.5) * size_x - center_x,
                    self.at(x, z) * height_scale,
                    (v - 0.5) * size_z - center_z,
                ]);
                let normal = self.sample_normal(u, v, height_scale);
                normals.push(normal);
                uvs.push([u, 1.0 - v]);
                match splat_mode {
                    VertexSplat::None => {}
                    VertexSplat::Palette => colors.push(terrain_splat_color(
                        self.at(x, z),
                        min_height,
                        max_height,
                        normal,
                    )),
                    VertexSplat::Weights => colors.push(terrain_splat_weights(
                        self.at(x, z),
                        min_height,
                        max_height,
                        normal,
                    )),
                }
            }
        }
        let mut indices = Vec::with_capacity((columns - 1) * (rows - 1) * 6);
        for z in 0..rows - 1 {
            for x in 0..columns - 1 {
                let top_left = (z * columns + x) as u32;
                let top_right = top_left + 1;
                let bottom_left = ((z + 1) * columns + x) as u32;
                let bottom_right = bottom_left + 1;
                indices.extend_from_slice(&[top_left, bottom_left, top_right]);
                indices.extend_from_slice(&[top_right, bottom_left, bottom_right]);
            }
        }
        MeshUploadInput {
            positions,
            normals,
            colors,
            uvs,
            tangents: if splat_mode == VertexSplat::Weights {
                vec![[1.0, 0.0, 0.0, 1.0]; columns * rows]
            } else {
                Vec::new()
            },
            indices,
        }
    }

    /// Apply one brush at normalized coords `(center_u, center_v)` (#517 Phase 2).
    ///
    /// `radius` also normalized to terrain width `[0, 1]`; `strength` for Raise/Lower is heightfield unit,
    /// for Smooth/Flatten is `0..1` blend factor. Cells outside radius
    /// are untouched, edge uses `(1 - t²)²` smooth falloff. Returns affected cell count.
    ///
    /// Non-finite / non-positive params treated as no-op; height won't produce NaN.
    pub fn sculpt(
        &mut self,
        center_u: f32,
        center_v: f32,
        radius: f32,
        strength: f32,
        mode: SculptMode,
    ) -> usize {
        prism_scene::terrain::sculpt_heights(
            self.cells,
            &mut self.heights,
            center_u,
            center_v,
            radius,
            strength,
            mode,
        )
    }

    /// Serialize back to `.raw` (f32 little-endian, row-major) — for CLI / editor writing disk.
    pub fn to_raw_bytes(&self) -> Vec<u8> {
        prism_scene::terrain::encode_heights(&self.heights)
    }
}

/// Sample index range for `[start, end]`: step by `step`, but always include both ends.
fn sample_indices(start: usize, end: usize, step: usize) -> Vec<usize> {
    let step = step.max(1);
    let mut indices = Vec::new();
    let mut index = start;
    while index < end {
        indices.push(index);
        index += step;
    }
    indices.push(end);
    indices
}

/// 4-layer palette for procedural color band splat (sand / grass / rock / snow).
const SPLAT_SAND: [f32; 3] = [0.78, 0.70, 0.49];
const SPLAT_GRASS: [f32; 3] = [0.30, 0.47, 0.21];
const SPLAT_ROCK: [f32; 3] = [0.43, 0.40, 0.36];
const SPLAT_SNOW: [f32; 3] = [0.90, 0.92, 0.96];

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix_color(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Height / slope → vertex color: sand low, grass mid, rock / snow high, steep slope favors rock.
fn terrain_splat_color(
    height: f32,
    min_height: f32,
    max_height: f32,
    normal: [f32; 3],
) -> [f32; 3] {
    let range = max_height - min_height;
    let height_t = if range.abs() < 1e-6 {
        0.5
    } else {
        ((height - min_height) / range).clamp(0.0, 1.0)
    };
    let base = if height_t < 0.35 {
        mix_color(SPLAT_SAND, SPLAT_GRASS, smoothstep(0.0, 0.35, height_t))
    } else if height_t < 0.7 {
        mix_color(SPLAT_GRASS, SPLAT_ROCK, smoothstep(0.35, 0.7, height_t))
    } else {
        mix_color(SPLAT_ROCK, SPLAT_SNOW, smoothstep(0.7, 1.0, height_t))
    };
    let slope = 1.0 - normal[1].clamp(0.0, 1.0);
    let rock = smoothstep(0.25, 0.6, slope);
    mix_color(base, SPLAT_ROCK, rock)
}

/// Height / slope → 4-layer texture weights, first three written to vertex color RGB, 4th = 1 - sum.
///
/// Layer order fixed as sand / grass / rock / snow, matching `Terrain.splat_materials` order;
/// steep slope shifts low-altitude sand / grass weight to rock.
fn terrain_splat_weights(
    height: f32,
    min_height: f32,
    max_height: f32,
    normal: [f32; 3],
) -> [f32; 3] {
    let range = max_height - min_height;
    let t = if range.abs() < 1e-6 {
        0.5
    } else {
        ((height - min_height) / range).clamp(0.0, 1.0)
    };
    let mut sand = (1.0 - smoothstep(0.0, 0.35, t)).max(0.0);
    let mut grass = smoothstep(0.0, 0.35, t) * (1.0 - smoothstep(0.5, 0.7, t));
    let snow = smoothstep(0.7, 0.95, t);
    let mut rock = (1.0 - sand - grass - snow).max(0.0);
    let slope = 1.0 - normal[1].clamp(0.0, 1.0);
    let rock_boost = smoothstep(0.25, 0.6, slope);
    let moved = (sand + grass) * rock_boost;
    sand *= 1.0 - rock_boost;
    grass *= 1.0 - rock_boost;
    rock += moved;
    let total = (sand + grass + rock + snow).max(1e-6);
    [sand / total, grass / total, rock / total]
}

/// In-process generated mesh table: path → vertex data (asset parser bypasses `.pak`).
#[derive(Debug, Default)]
pub struct ProceduralMeshes {
    meshes: HashMap<String, Arc<MeshUploadInput>>,
}

impl ProceduralMeshes {
    pub fn get(&self, path: &str) -> Option<MeshUploadInput> {
        self.meshes.get(path).map(|mesh| (**mesh).clone())
    }

    pub fn insert(&mut self, path: impl Into<String>, mesh: MeshUploadInput) {
        self.meshes.insert(path.into(), Arc::new(mesh));
    }

    pub fn remove(&mut self, path: &str) -> bool {
        self.meshes.remove(path).is_some()
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&String) -> bool) {
        self.meshes.retain(|path, _| keep(path));
    }
}

/// Runtime state of terrain entity: parsed heightfield + current generated mesh path.
///
/// When `key` changes (heightmap / size / height scale / resolution / entity changed) rebuild mesh.
#[derive(Debug, Clone)]
pub struct TerrainData {
    pub key: String,
    pub field: Arc<Heightfield>,
    /// Single-chunk mode is entity's own mesh; multi-chunk mode is chunk 0 (for debug / old tests).
    pub mesh_path: String,
    pub size: [f32; 2],
    pub height_scale: f32,
}

/// Runtime generated terrain chunk entity (#517 Phase 2).
///
/// Each chunk is a normal entity under `owner`, with own `MeshRenderer` / `MeshRef` /
/// `LocalTransform`; main camera and shadow frustum can cull per chunk. It has no `SceneEntityId`,
/// won't be saved by scene state.
#[derive(Debug, Clone)]
pub struct TerrainChunk {
    pub owner: prism_ecs::Entity,
    pub mesh_path: String,
}

/// A group of terrain chunks at one LOD level; `Child` holds all chunk entities of that level.
///
/// Whole terrain switches level uniformly (rather than each chunk separately), so adjacent chunks always use same step,
/// no cracks from differing boundary vertex density.
#[derive(Debug, Clone)]
pub struct TerrainLodSet {
    pub owner: prism_ecs::Entity,
    pub level: usize,
}

/// Runtime scattered terrain vegetation instance (#517 Phase 2).
#[derive(Debug, Clone)]
pub struct TerrainVegetation {
    pub owner: prism_ecs::Entity,
    pub mesh_path: String,
}

/// Runtime flag of texture layer splat: vertex color is 4-layer weights, tiling is UV multiplier.
#[derive(Debug, Clone, Copy)]
pub struct TerrainSplat {
    pub tiling: f32,
}

/// Terrain height (meters) at world coord `(x, z)`. Returns `None` if no terrain covers.
///
/// Only supports translated terrain (rotation axis-aligned, not interpolated); overlapping terrain takes highest.
pub fn terrain_height(world: &prism_ecs::World, x: f32, z: f32) -> Option<f32> {
    terrain_sample(world, x, z).map(|(_, height, _)| height)
}

/// Highest terrain at world coord `(x, z)`: entity, height (meters) and world normal.
///
/// Shares same resolution rule with [`terrain_height`]: only entity translation, highest when overlapping.
pub fn terrain_sample(
    world: &prism_ecs::World,
    x: f32,
    z: f32,
) -> Option<(prism_ecs::Entity, f32, [f32; 3])> {
    let mut best: Option<(prism_ecs::Entity, f32, [f32; 3])> = None;
    for (entity, data) in world.query::<TerrainData>() {
        let origin = world
            .get::<LocalToWorld>(entity)
            .map(|transform| transform.0.w_axis.truncate())
            .unwrap_or(glam::Vec3::ZERO);
        let half = glam::Vec2::new(data.size[0] * 0.5, data.size[1] * 0.5);
        let local = glam::Vec2::new(x - origin.x, z - origin.z);
        if local.x.abs() > half.x || local.y.abs() > half.y {
            continue;
        }
        let u = local.x / (half.x * 2.0) + 0.5;
        let v = local.y / (half.y * 2.0) + 0.5;
        let height = data.field.sample_height(u, v) * data.height_scale + origin.y;
        if best.is_none_or(|(_, current, _)| height > current) {
            let normal = data.field.sample_normal(u, v, data.height_scale);
            best = Some((entity, height, normal));
        }
    }
    best
}

/// One terrain raycast hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainHit {
    pub entity: prism_ecs::Entity,
    /// Distance from raycast origin (world units).
    pub distance: f32,
    /// World coord of hit point.
    pub point: [f32; 3],
    /// Heightfield normal at hit (world space, Y upward).
    pub normal: [f32; 3],
}

/// Raycast query against heightfield terrain (#517 Phase 2 item 4).
///
/// First sample terrain height at `(x, z)` along ray, find first crossing from above surface to below, then
/// binary refine; beyond `max_distance`, or ray origin already below surface, no hit returned.
///
/// Like [`terrain_height`] only supports **translated** terrain; overlapping terrain intersects highest one
/// (consistent with query interface). This is parsed query for gameplay, not physics engine continuous collision.
pub fn terrain_raycast(
    world: &prism_ecs::World,
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
) -> Option<TerrainHit> {
    let dir = glam::Vec3::from_array(direction);
    let length = dir.length();
    if !length.is_finite() || length <= 1e-6 || !max_distance.is_finite() || max_distance <= 0.0 {
        return None;
    }
    let dir = dir / length;
    let origin = glam::Vec3::from_array(origin);
    if !origin.is_finite() {
        return None;
    }

    let (_, start_height, _) = terrain_sample(world, origin.x, origin.z)?;
    if origin.y < start_height {
        // Origin already below surface (e.g. camera buried in mountain): no "crossing from inside" hit.
        return None;
    }

    // Coarse sample step: shorter ray finer, longer ray at most 512 samples; then 16 binary steps.
    let step = (max_distance / 512.0).clamp(0.25, 8.0).min(max_distance);
    let samples = ((max_distance / step).ceil() as usize).clamp(1, 4096);
    let mut previous: Option<(f32, f32)> = if origin.y > start_height {
        Some((0.0, origin.y - start_height))
    } else {
        None
    };

    for index in 1..=samples {
        let t = index as f32 / samples as f32 * max_distance;
        let point = origin + dir * t;
        let Some((_, height, _)) = terrain_sample(world, point.x, point.z) else {
            previous = None;
            continue;
        };
        let gap = point.y - height;
        if gap <= 0.0 {
            let lo = previous.map_or(0.0, |(previous_t, _)| previous_t);
            let (hit_t, entity, normal) = refine_terrain_hit(world, origin, dir, lo, t);
            let point = origin + dir * hit_t;
            return Some(TerrainHit {
                entity,
                distance: hit_t,
                point: point.to_array(),
                normal,
            });
        }
        previous = Some((t, gap));
    }
    None
}

/// Binary search between `[lo, hi]` for ray-terrain intersection (`lo` above surface, `hi` below).
fn refine_terrain_hit(
    world: &prism_ecs::World,
    origin: glam::Vec3,
    dir: glam::Vec3,
    mut lo: f32,
    mut hi: f32,
) -> (f32, prism_ecs::Entity, [f32; 3]) {
    let mut entity = none_entity();
    let mut normal = [0.0, 1.0, 0.0];
    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        let point = origin + dir * mid;
        let Some((sample_entity, height, sample_normal)) = terrain_sample(world, point.x, point.z)
        else {
            break;
        };
        entity = sample_entity;
        normal = sample_normal;
        if point.y > height {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (hi, entity, normal)
}

/// Placeholder entity while binary search hasn't sampled yet (normally always covered by real sample).
fn none_entity() -> prism_ecs::Entity {
    prism_ecs::Entity::from_raw(u32::MAX, u32::MAX)
}

/// Mesh parameter fingerprint of `Terrain`: change means rebuild mesh.
pub fn terrain_key(entity_id: u32, terrain: &Terrain) -> String {
    format!(
        "{entity_id}|{}|{}x{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}",
        terrain.heightmap,
        terrain.size[0],
        terrain.size[1],
        terrain.height_scale,
        terrain.cells(),
        terrain.chunks,
        terrain.lod_distances,
        terrain.vegetation_density,
        terrain.vegetation_mesh,
        terrain.vegetation_material,
        terrain.vegetation_size,
        terrain.splat,
        terrain.splat_materials.join(","),
        terrain.splat_tiling
    )
}

/// Read heightmap (`.raw`, f32 LE). Path resolution consistent with other assets; returns `None` on failure and caller warns.
pub fn load_heightfield(terrain: &Terrain) -> Option<Heightfield> {
    if !terrain.has_heightmap() {
        return Some(Heightfield::flat(terrain.cells()));
    }
    let Some(path) = crate::asset_paths::asset_path(&terrain.heightmap) else {
        log::warn!(
            "terrain: cannot resolve asset root for heightmap {}; using a flat field",
            terrain.heightmap
        );
        return Some(Heightfield::flat(terrain.cells()));
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!(
                "terrain: cannot read heightmap {} ({error}); using a flat field",
                path.display()
            );
            return Some(Heightfield::flat(terrain.cells()));
        }
    };
    // `resolution = 0` = follow file size; otherwise must match exactly, avoid drawing wrong terrain.
    let parsed = if terrain.resolution == 0 {
        Heightfield::from_raw_auto(&bytes)
    } else {
        Heightfield::from_raw(&bytes, terrain.cells())
    };
    match parsed {
        Some(field) => Some(field),
        None => {
            log::warn!(
                "terrain: heightmap {} is {} bytes, expected {} ({} cells); using a flat field",
                path.display(),
                bytes.len(),
                terrain.cells() * terrain.cells() * 4,
                terrain.cells()
            );
            Some(Heightfield::flat(terrain.cells()))
        }
    }
}

/// Turn `Terrain` entity into renderable mesh (#517).
///
/// Rebuild only when parameter fingerprint changes: (re)generate `MeshUploadInput` → register into
/// [`ProceduralMeshes`] → point same entity's `MeshRenderer` to it, clear `MeshRef` generation,
/// hand to asset parser for upload. Material uses path authored on MeshRenderer.
pub fn terrain_system(world: &mut prism_ecs::World) {
    cleanup_orphan_chunks(world);

    let mut pending = Vec::new();
    for (entity, terrain) in world.query::<Terrain>() {
        let key = terrain_key(entity.id(), terrain);
        let up_to_date = world
            .get::<TerrainData>(entity)
            .is_some_and(|data| data.key == key);
        if !up_to_date {
            pending.push((entity, key, terrain.clone()));
        }
    }
    for (entity, key, terrain) in pending {
        rebuild_terrain(world, entity, key, terrain);
    }
    apply_terrain_lod(world);
}

/// Cell boundary for given chunk count per side: length `chunks + 1`, at least one quad per chunk.
fn chunk_bounds(cells: usize, chunks: usize) -> Vec<usize> {
    let segments = cells.saturating_sub(1);
    (0..=chunks)
        .map(|index| index * segments / chunks)
        .collect()
}

/// Recycle all chunk entities and procedural meshes under some terrain owner name.
fn despawn_chunks(world: &mut prism_ecs::World, owner: prism_ecs::Entity) {
    let chunks: Vec<(prism_ecs::Entity, String)> = world
        .query::<TerrainChunk>()
        .filter(|(_, chunk)| chunk.owner == owner)
        .map(|(entity, chunk)| (entity, chunk.mesh_path.clone()))
        .collect();
    let sets: Vec<prism_ecs::Entity> = world
        .query::<TerrainLodSet>()
        .filter(|(_, set)| set.owner == owner)
        .map(|(entity, _)| entity)
        .collect();
    if chunks.is_empty() && sets.is_empty() {
        return;
    }
    let mut stale_ids: HashSet<prism_ecs::Entity> =
        chunks.iter().map(|(entity, _)| *entity).collect();
    stale_ids.extend(sets.iter().copied());
    if let Some(meshes) = world.get_resource_mut::<ProceduralMeshes>() {
        for (_, path) in &chunks {
            meshes.remove(path);
        }
    }
    if world.is_alive(owner) {
        if let Some(children) = world.get_mut::<Child>(owner) {
            children.0.retain(|entity| !stale_ids.contains(entity));
        }
        if world
            .get::<Child>(owner)
            .is_some_and(|children| children.0.is_empty())
        {
            world.remove::<Child>(owner);
        }
    }
    for (entity, _) in chunks {
        world.despawn(entity);
    }
    for entity in sets {
        world.despawn(entity);
    }
}

/// Recycle all vegetation instances under some terrain owner name.
fn despawn_vegetation(world: &mut prism_ecs::World, owner: prism_ecs::Entity) {
    let stale: Vec<(prism_ecs::Entity, String)> = world
        .query::<TerrainVegetation>()
        .filter(|(_, plant)| plant.owner == owner)
        .map(|(entity, plant)| (entity, plant.mesh_path.clone()))
        .collect();
    if stale.is_empty() {
        return;
    }
    let stale_ids: HashSet<prism_ecs::Entity> = stale.iter().map(|(entity, _)| *entity).collect();
    if let Some(meshes) = world.get_resource_mut::<ProceduralMeshes>() {
        for (_, path) in &stale {
            meshes.remove(path);
        }
    }
    if world.is_alive(owner) {
        if let Some(children) = world.get_mut::<Child>(owner) {
            children.0.retain(|entity| !stale_ids.contains(entity));
        }
        if world
            .get::<Child>(owner)
            .is_some_and(|children| children.0.is_empty())
        {
            world.remove::<Child>(owner);
        }
    }
    for (entity, _) in stale {
        world.despawn(entity);
    }
}

/// After terrain entity deleted, clear leftover chunks / vegetation (instances without owner won't disappear on their own).
fn cleanup_orphan_chunks(world: &mut prism_ecs::World) {
    let live: HashSet<prism_ecs::Entity> =
        world.query::<Terrain>().map(|(entity, _)| entity).collect();
    let stale: HashSet<prism_ecs::Entity> = world
        .query::<TerrainChunk>()
        .map(|(_, chunk)| chunk.owner)
        .chain(world.query::<TerrainLodSet>().map(|(_, set)| set.owner))
        .chain(
            world
                .query::<TerrainVegetation>()
                .map(|(_, plant)| plant.owner),
        )
        .filter(|owner| !live.contains(owner))
        .collect();
    for owner in stale {
        despawn_chunks(world, owner);
        despawn_vegetation(world, owner);
    }
}

/// Choose LOD level by distance from nearest enabled camera to terrain origin, whole terrain switches level uniformly.
fn apply_terrain_lod(world: &mut prism_ecs::World) {
    let eyes: Vec<glam::Vec3> = world
        .query2::<Camera, LocalToWorld>()
        .filter(|(_, camera, _)| camera.enabled)
        .map(|(_, _, world)| world.0.w_axis.truncate())
        .collect();

    let terrains: Vec<(prism_ecs::Entity, Vec<f32>, glam::Vec3)> = world
        .query2::<Terrain, LocalToWorld>()
        .map(|(entity, terrain, world)| {
            (
                entity,
                terrain.lod_distances.clone(),
                world.0.w_axis.truncate(),
            )
        })
        .collect();
    // Terrain without world matrix yet (just spawned): distance by origin.
    let terrains_without_transform: Vec<(prism_ecs::Entity, Vec<f32>)> = world
        .query::<Terrain>()
        .filter(|(entity, _)| world.get::<LocalToWorld>(*entity).is_none())
        .map(|(entity, terrain)| (entity, terrain.lod_distances.clone()))
        .collect();

    let mut levels: HashMap<prism_ecs::Entity, usize> = HashMap::new();
    for (entity, distances, position) in terrains {
        levels.insert(entity, lod_level(&distances, position, &eyes));
    }
    for (entity, distances) in terrains_without_transform {
        levels.insert(entity, lod_level(&distances, glam::Vec3::ZERO, &eyes));
    }
    if levels.is_empty() {
        return;
    }

    let sets: Vec<(prism_ecs::Entity, prism_ecs::Entity, usize)> = world
        .query::<TerrainLodSet>()
        .map(|(entity, set)| (entity, set.owner, set.level))
        .collect();
    for (set_entity, owner, set_level) in sets {
        let visible = levels.get(&owner).copied().unwrap_or(0) == set_level;
        let children: Vec<prism_ecs::Entity> = world
            .get::<Child>(set_entity)
            .map(|children| children.0.clone())
            .unwrap_or_default();
        for child in children {
            world.insert(child, crate::scene::components::LodHidden(!visible));
        }
    }
}

/// Distance → LOD level: first threshold >= distance; beyond all thresholds use last level.
fn lod_level(distances: &[f32], position: glam::Vec3, eyes: &[glam::Vec3]) -> usize {
    if distances.is_empty() {
        return 0;
    }
    let distance = eyes
        .iter()
        .map(|eye| (*eye - position).length())
        .fold(f32::INFINITY, f32::min);
    let distance = if distance.is_finite() {
        distance.max(0.0)
    } else {
        0.0
    };
    distances
        .iter()
        .position(|limit| {
            let limit = if limit.is_finite() {
                limit.max(0.0)
            } else {
                f32::INFINITY
            };
            distance <= limit
        })
        .unwrap_or(distances.len())
        .min(2)
}

/// Ensure entity enters "waiting asset parse" state.
fn ensure_mesh_ref(world: &mut prism_ecs::World, entity: prism_ecs::Entity) {
    match world.get_mut::<MeshRef>(entity) {
        Some(reference) => reference.generation = 0,
        None => {
            world.insert(
                entity,
                MeshRef {
                    asset_id: SceneAssetId::generate(),
                    render_handle: MeshHandle::default(),
                    generation: 0,
                },
            );
        }
    }
}

/// Double-sided cross-quad: height 1 m, width 1 m, bottom at y = 0, for vegetation without custom mesh.
fn vegetation_cross_mesh() -> MeshUploadInput {
    let positions = vec![
        [-0.5, 0.0, 0.0],
        [0.5, 0.0, 0.0],
        [0.5, 1.0, 0.0],
        [-0.5, 1.0, 0.0],
        [0.0, 0.0, -0.5],
        [0.0, 0.0, 0.5],
        [0.0, 1.0, 0.5],
        [0.0, 1.0, -0.5],
    ];
    let mut normals = vec![[0.0, 0.0, 1.0]; 4];
    normals.extend_from_slice(&[[1.0, 0.0, 0.0]; 4]);
    let uvs = vec![
        [0.0, 0.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
        [0.0, 0.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
    ];
    MeshUploadInput {
        positions,
        normals,
        colors: Vec::new(),
        uvs,
        tangents: Vec::new(),
        indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
    }
}

/// Stable 64-bit hash (vegetation scattering / variant seed).
fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// Scatter vegetation instances deterministically by heightfield (#517 Phase 2).
///
/// All instances share same mesh path, handed to renderer static instancing batching; steep slopes (normal Y < 0.55)
/// skipped. Without custom mesh generate double-sided cross-quad.
fn spawn_vegetation(
    world: &mut prism_ecs::World,
    owner: prism_ecs::Entity,
    key: &str,
    hash: u64,
    field: &Heightfield,
    terrain: &Terrain,
    renderer_template: &MeshRenderer,
) {
    let density = terrain.vegetation_density;
    if !density.is_finite() || density <= 0.0 {
        return;
    }
    let size_x = terrain.size[0].max(0.001);
    let size_z = terrain.size[1].max(0.001);
    let area = size_x * size_z;
    let count = (density * area / 100.0).round() as usize;
    let count = count.min(4096);
    if count == 0 {
        return;
    }

    let custom = terrain.vegetation_mesh.trim();
    let mesh_path = if custom.is_empty() {
        let path = format!(
            "{PROCEDURAL_PREFIX}terrain/{}-{hash:x}/vegetation",
            owner.id()
        );
        if let Some(meshes) = world.get_resource_mut::<ProceduralMeshes>() {
            meshes.insert(path.clone(), vegetation_cross_mesh());
        }
        path
    } else {
        custom.to_string()
    };
    let plant_size = if terrain.vegetation_size.is_finite() && terrain.vegetation_size > 0.0 {
        terrain.vegetation_size
    } else {
        1.0
    };
    let material = terrain.vegetation_material.trim();
    let half = [size_x * 0.5, size_z * 0.5];
    let seed = fnv1a64(key);

    for index in 0..count {
        let first = splitmix64(seed ^ index as u64);
        let second = splitmix64(first);
        let u = (first & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32;
        let v = (second & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32;
        let normal = field.sample_normal(u, v, terrain.height_scale);
        if normal[1] < 0.55 {
            continue;
        }
        let height = field.sample_height(u, v) * terrain.height_scale;
        let yaw = ((second >> 24) & 0xFFFF) as f32 / 0xFFFF as f32 * std::f32::consts::TAU;
        let scale = plant_size * (0.75 + ((first >> 48) & 0xFF) as f32 / 255.0 * 0.5);

        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                position: glam::Vec3::new(
                    (u - 0.5) * 2.0 * half[0],
                    height,
                    (v - 0.5) * 2.0 * half[1],
                ),
                rotation: glam::Quat::from_rotation_y(yaw),
                scale: glam::Vec3::splat(scale),
            },
        );
        let mut renderer = renderer_template.clone();
        renderer.mesh_path = mesh_path.clone();
        renderer.material_path = material.to_string();
        renderer.material_paths.clear();
        renderer.cull_mode = CullMode::Off;
        renderer.shadow_casting_mode = ShadowCastingMode::Off;
        world.insert(entity, renderer);
        world.insert(
            entity,
            MeshRef {
                asset_id: SceneAssetId::generate(),
                render_handle: MeshHandle::default(),
                generation: 0,
            },
        );
        world.insert(
            entity,
            TerrainVegetation {
                owner,
                mesh_path: mesh_path.clone(),
            },
        );
        HierarchyHelper::reparent(world, entity, Some(owner));
    }
}

/// Ensure entity enters "waiting material parse" state.
fn ensure_material_ref(world: &mut prism_ecs::World, entity: prism_ecs::Entity) {
    if world.get::<MaterialRef>(entity).is_none() {
        world.insert(
            entity,
            MaterialRef {
                asset_id: SceneAssetId::generate(),
                material_slot: 0,
                generation: 0,
            },
        );
    }
}

/// Write `Terrain.splat_materials` into renderer's multiple material paths (up to 4 layers).
fn apply_splat_renderer(renderer: &mut MeshRenderer, terrain: &Terrain) {
    let layers: Vec<String> = terrain
        .splat_materials
        .iter()
        .take(4)
        .filter(|path| !path.trim().is_empty())
        .cloned()
        .collect();
    if let Some(first) = layers.first() {
        renderer.material_path = first.clone();
        renderer.material_paths = layers;
    }
}

/// Rebuild one terrain's mesh: one whole piece, or `chunks × chunks` independent child entities.
fn rebuild_terrain(
    world: &mut prism_ecs::World,
    entity: prism_ecs::Entity,
    key: String,
    terrain: Terrain,
) {
    let Some(field) = load_heightfield(&terrain) else {
        return;
    };
    despawn_chunks(world, entity);
    despawn_vegetation(world, entity);
    world.remove::<TerrainSplat>(entity);

    // Old mesh no longer referenced, recycle it too.
    if let Some(previous) = world.get::<TerrainData>(entity) {
        let previous_path = previous.mesh_path.clone();
        if !previous_path.is_empty() {
            if let Some(meshes) = world.get_resource_mut::<ProceduralMeshes>() {
                meshes.remove(&previous_path);
            }
        }
    }
    if world.get_resource_mut::<ProceduralMeshes>().is_none() {
        world.insert_resource(ProceduralMeshes::default());
    }

    let size = terrain.size;
    let height_scale = terrain.height_scale;
    let hash = fnv1a64(&key);
    // At least one quad per chunk; small mesh (cells = 2) degrades to single chunk.
    let chunks = terrain.chunk_count().min(field.cells() - 1).max(1);

    let levels = terrain.lod_level_count();
    let texture_splat = !terrain.splat_materials.is_empty();
    let splat_tiling = if terrain.splat_tiling.is_finite() && terrain.splat_tiling > 0.0 {
        terrain.splat_tiling
    } else {
        16.0
    };
    if chunks == 1 && levels == 1 {
        let path = format!("{PROCEDURAL_PREFIX}terrain/{}-{hash:x}", entity.id());
        let mesh = if texture_splat {
            field.build_mesh_with_weights(size, height_scale)
        } else {
            field.build_mesh_with_splat(size, height_scale, terrain.splat)
        };
        if let Some(meshes) = world.get_resource_mut::<ProceduralMeshes>() {
            meshes.insert(path.clone(), mesh);
        }

        // MeshRenderer is author component: only change path, keep material and shadow settings.
        match world.get_mut::<MeshRenderer>(entity) {
            Some(renderer) => renderer.mesh_path = path.clone(),
            None => {
                world.insert(
                    entity,
                    MeshRenderer {
                        mesh_path: path.clone(),
                        ..Default::default()
                    },
                );
            }
        }
        if let Some(renderer) = world.get_mut::<MeshRenderer>(entity) {
            apply_splat_renderer(renderer, &terrain);
        }
        ensure_mesh_ref(world, entity);
        ensure_material_ref(world, entity);
        if texture_splat {
            world.insert(
                entity,
                TerrainSplat {
                    tiling: splat_tiling,
                },
            );
        }
        let renderer_template = world
            .get::<MeshRenderer>(entity)
            .cloned()
            .unwrap_or_default();
        spawn_vegetation(
            world,
            entity,
            &key,
            hash,
            &field,
            &terrain,
            &renderer_template,
        );
        world.insert(
            entity,
            TerrainData {
                key,
                field: Arc::new(field),
                mesh_path: path,
                size,
                height_scale,
            },
        );
        return;
    }

    // Multi-chunk / LOD: owner only keeps material / shadow settings, no longer draws directly.
    // Each LOD level is a set entity, under it all chunks of that level.
    if let Some(renderer) = world.get_mut::<MeshRenderer>(entity) {
        apply_splat_renderer(renderer, &terrain);
    }
    let renderer_template = world
        .get::<MeshRenderer>(entity)
        .cloned()
        .unwrap_or_default();
    let bounds = chunk_bounds(field.cells(), chunks);
    let last = (field.cells() - 1) as f32;
    let size_x = size[0].max(0.001);
    let size_z = size[1].max(0.001);
    let mut first_path = String::new();

    for level in 0..levels {
        let step = 1usize << level;
        let set_entity = world.spawn();
        world.insert(set_entity, LocalTransform::default());
        world.insert(
            set_entity,
            TerrainLodSet {
                owner: entity,
                level,
            },
        );
        HierarchyHelper::reparent(world, set_entity, Some(entity));

        for z in 0..chunks {
            for x in 0..chunks {
                let path = format!(
                    "{PROCEDURAL_PREFIX}terrain/{}-{hash:x}/l{level}/c{x}_{z}",
                    entity.id()
                );
                let mesh = if texture_splat {
                    field.build_chunk_mesh_lod_with_weights(
                        size,
                        height_scale,
                        bounds[x],
                        bounds[x + 1],
                        bounds[z],
                        bounds[z + 1],
                        step,
                    )
                } else {
                    field.build_chunk_mesh_lod_with_splat(
                        size,
                        height_scale,
                        bounds[x],
                        bounds[x + 1],
                        bounds[z],
                        bounds[z + 1],
                        step,
                        terrain.splat,
                    )
                };
                if let Some(meshes) = world.get_resource_mut::<ProceduralMeshes>() {
                    meshes.insert(path.clone(), mesh);
                }
                if first_path.is_empty() {
                    first_path = path.clone();
                }

                let center_x = (((bounds[x] + bounds[x + 1]) as f32 * 0.5 / last) - 0.5) * size_x;
                let center_z = (((bounds[z] + bounds[z + 1]) as f32 * 0.5 / last) - 0.5) * size_z;
                let chunk_entity = world.spawn();
                world.insert(
                    chunk_entity,
                    LocalTransform {
                        position: glam::Vec3::new(center_x, 0.0, center_z),
                        ..Default::default()
                    },
                );
                let mut renderer = renderer_template.clone();
                renderer.mesh_path = path.clone();
                world.insert(chunk_entity, renderer);
                world.insert(
                    chunk_entity,
                    MeshRef {
                        asset_id: SceneAssetId::generate(),
                        render_handle: MeshHandle::default(),
                        generation: 0,
                    },
                );
                ensure_material_ref(world, chunk_entity);
                if texture_splat {
                    world.insert(
                        chunk_entity,
                        TerrainSplat {
                            tiling: splat_tiling,
                        },
                    );
                }
                world.insert(
                    chunk_entity,
                    TerrainChunk {
                        owner: entity,
                        mesh_path: path,
                    },
                );
                HierarchyHelper::reparent(world, chunk_entity, Some(set_entity));
            }
        }
    }

    spawn_vegetation(
        world,
        entity,
        &key,
        hash,
        &field,
        &terrain,
        &renderer_template,
    );

    // Owner no longer has MeshRef, so won't be drawn again in main pass.
    world.remove::<MeshRef>(entity);
    if let Some(renderer) = world.get_mut::<MeshRenderer>(entity) {
        renderer.mesh_path.clear();
    }
    world.insert(
        entity,
        TerrainData {
            key,
            field: Arc::new(field),
            mesh_path: first_path,
            size,
            height_scale,
        },
    );
}

/// FNV-1a: short fingerprint in mesh path (no crypto strength needed, just stable).
fn fnv1a64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
#[path = "terrain_tests.rs"]
mod tests;
