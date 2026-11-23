# Emergence: v3.0 ultimate panoramic tag system — Prisma engine development plan

> Project directory: `projects/emergence/` (independent workspace, same level as `projects/template`, `projects/echowell`, `projects/fishing`).
> Architecture baseline: strictly follows repo `AGENTS.md` / `GEMINI.md`, `docs/DESIGN.md`, `docs/REFLECT.md` and `docs/project-module-architecture.md` layering red lines and zero-compatibility-before-1.0 principle.

---

## 1. Project positioning and engine / project layering hard boundary

`Emergence` is a physics and hazardous-chemical sandbox for hardcore emergent interaction. This development plan centers on **evolution of Prisma engine base capabilities**, breaking v3.0 five major tag systems (base environment state enums, intrinsic material bits, transient hazard bits, spatial topology bits, external stimuli and dynamic clock) into reusable general engine infrastructure and hot-swappable project rule tables.

### 1.1 Hard Boundary Contract

Criterion: **if switching to another chemistry / ecology / industrial simulation sandbox, can this capability be reused as-is?** Yes → engine `crates/prism-*`; no → stay in `projects/emergence/`.

| Layer | Crate | Allowed responsibilities | Forbidden responsibilities |
|---|---|---|---|
| **Core scheduling and concurrency layer** | `prism-ecs` | Active set / dormant skip (`DORMANT` zero-cost iteration), Chunk parallel jobs (`jobs::scoped_parallel_chunks`), multi-rate fixed-step scheduler | Concrete physics formulas like heat conduction, acid corrosion, radiation, bioactivity |
| **Reflection and scene protocol layer** | `prism-reflect` / `prism-scene` | `Bitflags` field metadata descriptions, compact binary / RSCN v4 component codec, stable `TypeId`, RTTIBin cross-boundary transfer | Hardcoding bit semantics of 15 materials or 18 transient states |
| **Spatial geometry and topology core** | `prism-physics2d` / `rapier3d` (`prism-app/physics`) | Deterministic AABB collision (`Aabb2`, `raycast_aabb`), grid adjacency queries, connected-domain / structural-support Flood-Fill, compound rigid bodies and dynamic voxel islands | Writing vacuum boiling, high-pressure liquefaction or bio-parasite spread logic |
| **Engine runtime service layer** | `prism-engine` / `prism-app` | Asset root paths (`asset_paths`), `DirtyRouter` inter-frame dirty-data routing, hot reload, audio driver and time-division driver | Any ROM substance tables or concrete gameplay state machines |
| **Render and visual feedback layer** | `prism-render` / Slang | `SpritePass` 2D sprite batching (`SpriteInstance::flags`), **Compute DDA three-level voxel raymarching**, lights and particles, debug overlay | Hardcoded render branches by material name |
| **Editor and automation layer** | `prism-editor-api` / `prism-editor-mcp` | Bitflag fold inspector UI Schema projection, headless stimulus injection and property assertion tools | Linking `prism-render` / Vulkan |
| **User project layer** | `projects/emergence/` | `PhysicalCell` 16-byte core, v3.0 5-dimension transition rule matrix, ROM substance config tables, lab scenes and interaction probes | Direct `ash::vk`, bypassing `GameWorld` / `HostApi` to access render thread, or introducing egui dependency |

---

## 2. Underlying compact memory model and 3D three-level voxel topology

To support zero-allocation simulation of massive grids and 3D fine destruction at 60 Hz, v3.0 panoramic tag system doesn't use scattered heap-allocated components or string tags, but uniformly packs into **16-byte `#[repr(C)]` `PhysicalCell`** (implemented in [`projects/emergence/src/tags.rs`](src/tags.rs)): a single 64-byte CPU cache line holds **4 complete physical cells** contiguously.

Spatially, the **16-byte `PhysicalCell` serves as the minimal atomic unit (L2) for interaction and physical simulation**, forming a **L1 (1 m) → L2 (0.25 m) → L3 (0.0625 m)** three-level voxel division with upper and lower levels (Region handles world-scale streaming):

### 2.0 3D three-level voxel hierarchy definition (1 m / 0.25 m / 0.0625 m)

| Level | Spatial scale | Data form | Core responsibility |
|---|---|---|---|
| **L1: World and terrain base (1 m)** | `1 m` cube<br>(contains `4×4×4` L2) | Macro index / `u64` active and airtight bitmask + **copy-on-write split (COW)** | Character base collision, room airtight connected domains and streaming units. Undamaged terrain blocks store only material ID, no L2 allocated; on excavation / blasting / acid corrosion split into L2 bricks on demand |
| **L2: Minimal interaction entity core (0.25 m)** | covers `4×4×4 = 64` cells / L1<br>(contains `4×4×4` L3) | **`64 × 16 B = 1 KB` `PhysicalCell` brick** | **Only minimal unit of v3.0 61-dimension tag simulation**: carries `CONDUIT` / `VALVE` / `CONVEYOR` / `RIGID_WELD` interactions, phase changes, electrothermal acid corrosion, bio evolution and structural load-bearing connectivity |
| **L3: Decoration, connection and particle layer (0.0625 m)** | `1/16 m` micro-voxel | Lightweight visual placeholder and palette index (**doesn't run 16B tag physics**) | Pure building micro-carving, pipe corner connections, destruction rough edges and splash particles: freely place / destroy, **saving 64× logic compute directly** |

> **Two cross-layer iron rules**:
> 1. **Prevent ghost wall writeback**: when all L3 in an L2 cell are broken away (`l3_occupied_mask == 0`), must synchronously clear that L2 cell's entity occupancy and `AIRTIGHT` flag;
> 2. **Transient shading inheritance**: L3 doesn't duplicate state bits, at render time directly reads parent L2's `TransientFlags` (e.g. `BURNING`, `FROZEN`, `CORRODED`, `WARNING`) for visual overlay effects.

### 2.1 Memory layout overview (Exact 16 Bytes)

```text
Offset   Size  Field              Packed Content
────────────────────────────────────────────────────────────────────────────
0x00     2 B   env: EnvPacked     Matter(2b) | Temp(3b) | Press(3b) | Density(3b) | Bio(2b) | FlowDir(3b)
0x02     2 B   material           15 Intrinsic Material & Physical Bits (u16)
0x04     4 B   transient          18 Dynamic Transient & Hazard Bits (u32)
0x08     2 B   topology           10 Spatial Topology & Affordance Bits (u16)
0x0A     1 B   pending_stimuli    8 Base Stimulus Input Channels (u8)
0x0B     1 B   durability         Structural integrity counter (0..=255)
0x0C     4 B   clocks             HALF_LIFE(15b) | SWITCH_HEAT(8b) | INTENSITY(4b) | TICK_HZ(5b)
────────────────────────────────────────────────────────────────────────────
Total = 16 bytes (const assert size_of::<PhysicalCell>() == 16)
```

### 2.2 Five-dimension bit-level mapping

1. **Core state and environment enums (`EnvPacked(u16)` — exactly fills 16 bits)**:
   - `MatterState` (bits `0..2`): `SOLID (0)` / `LIQUID (1)` / `GAS (2)` / `PLASMA (3)`
   - `Temperature` (bits `2..5`): `ULTRA_CRYO (0)` / `CRYO (1)` / `AMBIENT (2)` / `HOT (3)` / `PLASMA_HEAT (4)`
   - `Pressure` (bits `5..8`): `HARD_VACUUM (0)` / `VACUUM (1)` / `NORMAL (2)` / `PRESSURIZED (3)` / `GIGA_PRESSURE (4)`
   - `Density` (bits `8..11`): 3-bit discrete steps `0..=7`
   - `BioActivity` (bits `11..13`): `INERT (0)` / `CRYPTOSIS (1)` / `VITAL (2)` / `HYPER_NEO (3)`
   - `FlowDir` (bits `13..16`): `NONE (0)` / `N (1)` / `S (2)` / `E (3)` / `W (4)` / `IN (5)` / `OUT (6)` / `BI_DIR (7)`
2. **Intrinsic material bits (`MaterialFlags(u16)` — 15 bits)**:
   `METALLIC`, `ORGANIC`, `GLASS`, `CONDUCTIVE`, `INSULATED`, `FLAMMABLE`, `AIRTIGHT`, `PRESSURE_RATED`, `HOT_RESISTANT`, `RAD_SHIELDED`, `PIEZOELECTRIC`, `SUPERCONDUCTIVE`, `ELASTIC`, `ABSORBENT`, `OXIDIZER`.
3. **Transient hazard bits (`TransientFlags(u32)` — 18 bits)**:
   `WET`, `BURNING`, `FROZEN`, `CHARGED`, `MAGNETIZED`, `BRITTLE`, `CORRODED`, `BROKEN`, `VOLATILE`, `TOXIC`, `CORROSIVE`, `RADIOACTIVE`, `INFECTED`, `STASIS`, `WARNING`, `DORMANT`, `SHARP`, `VISCOUS`.
4. **Spatial topology bits (`TopologyFlags(u16)` — 10 bits)**:
   `RIGID_WELD`, `BIO_NEST`, `FROZEN_LOCKED`, `PUNCTURED`, `EDIBLE`, `EQUIPPABLE`, `SEALED`, `CONVEYOR`, `CONDUIT`, `VALVE`.
5. **External stimuli and dynamic clock (`StimulusFlags(u8)` + `SignalClockState(u32)`)**:
   - 8 base stimuli: `HEAT`, `COLD`, `SHOCK`, `ACID`, `BIO`, `KINETIC`, `RADIATION`, `LIGHT`.
   - 4 registers: `HALF_LIFE` (0..=32767, 15 bits), `SWITCH_HEAT` (0..=255, 8 bits), `INTENSITY` (0..=15, 4 bits), `TICK_HZ` (0..=20, 5 bits).

---

## 3. Core physical invariants and emergent transition mechanism design

### 3.1 Matter-state forced density ladder and temperature-pressure phase diagram

- **Macro density suppression iron rule**: `EnvPacked::new` / `set_matter` / `set_density_hint` force clamping at write time:
  - `GAS` / `PLASMA` locked at `0..=1` (lightest tier, material fine-tuning within tier like hydrogen vs sulfur hexafluoride);
  - `LIQUID` locked at `2..=4` (medium tier);
  - `SOLID` locked at `5..=7` (heaviest tier).
  This eliminates "gas heavier than solid" buoyancy inversion and sorting artifacts at the data-structure level, naturally driving buoyancy layering, pump-free thermal convection and 4–5 Hz centrifugal separation.
- **Temperature-pressure coupled phase transitions**:
  - Vacuum boiling: `LIQUID` under `VACUUM | HARD_VACUUM` with temperature `>= AMBIENT` forced to vaporize into `GAS`;
  - High-pressure liquefaction: `GAS` under `GIGA_PRESSURE` with temperature `<= AMBIENT` forced to compress into `LIQUID`;
  - Low-temperature freezing: `COLD` pushes temperature to `CRYO | ULTRA_CRYO`, liquid solidifies into `SOLID` + `FROZEN`; non-`ELASTIC` materials also get `BRITTLE` (shatter into `BROKEN` on `KINETIC` impact).

### 3.2 Bioactivity four-stage state machine (`BioActivity`)

- `INERT`: baseline for dead matter or non-organisms, no metabolism or infection spread;
- `CRYPTOSIS`: cryptobiotic dormant state — `ORGANIC` + `VITAL` entering `CRYO/ULTRA_CRYO` or `FROZEN` auto-transitions to `CRYPTOSIS`, freezing `HALF_LIFE` decay clock; revives to `VITAL` on return to `AMBIENT` + `WET`;
- `VITAL`: living state — continuously advances `HALF_LIFE` metabolism clock; if simultaneously exposed to `AMBIENT` + `WET` + `BIO` stimulus without `SEALED` protection, triggers `INFECTED` (hyphal parasitism), transforming into `BIO_NEST` producing resin when conditions ripen;
- `HYPER_NEO`: frenzied mutated state — `VITAL` exposed to unshielded `RAD_SHIELDED` `RADIATION` stimulus mutates, ignoring normal metabolism throttling and emitting `BIO` / `TOXIC` stimuli outward.

### 3.3 Hazard warning and zero-cost dormancy mechanism

- **`WARNING` rescue window**: when high-pressure vessel approaches `PRESSURE_RATED` limit, superconducting coil about to quench (temperature crossing `CRYO`) or reactor approaching criticality, force a **3–5 second** `WARNING` state, lighting shader high-frequency flashing alarm and audio channel alarm.
- **`STASIS` stasis field**: while `TransientFlags::STASIS` set, all phase changes, chemical reactions, stimulus consumption and `HALF_LIFE` decrement frozen.
- **`DORMANT` dormant pruning**: cell auto-sets `DORMANT` and is removed from active simulation list when no `pending_stimuli`, no active `HALF_LIFE` / `SWITCH_HEAT` timers, and no `BURNING` / `WARNING` / `INFECTED` persistent hazards; wakes immediately on external `apply_stimulus` injection or neighbor dirty-bit write.
- **`SWITCH_HEAT` oscillation fuse**: non-`SUPERCONDUCTIVE` conductor on low→high transition (`!CHARGED -> CHARGED`) increments `switch_heat`, upon reaching `SWITCH_HEAT_MELTDOWN_THRESHOLD` immediately burns into `BROKEN` and clears `CHARGED`, physically blocking CPU infinite loops from high-frequency redstone oscillation.

### 3.4 Spatial topology and transport pipe network

- **Surface conveyor (`CONVEYOR`)**: moves solid entities along `FlowDir` at `TICK_HZ (0..=20)` discrete steps; when overclocked (near `20 Hz`) assigns projectile velocity vector at exit (catapult gameplay).
- **Sealed pipe (`CONDUIT` + `VALVE`)**: transports fluids and gases, driven by connected-domain pressure differences and same-temperature density differences (pump-free thermal convection); `VALVE` controlled by `CHARGED` state; liquid inside pipe triggering `FROZEN` or getting stuck in `VISCOUS` blockage causes upstream pressure spike and `WARNING` → `PUNCTURED` / rupture.
- **Compound rigid bodies, structural collapse and container topology (`RIGID_WELD` / `Teardown-style destruction` / `FROZEN_LOCKED` / `PUNCTURED` / `SEALED` / `EDIBLE` / `EQUIPPABLE`)**:
  - **Structural connectivity break (Teardown-Style Destruction)**: when L2 voxel is blasted by `KINETIC`, corroded through by `ACID` (`CORRODED`), shattered by `COLD` embrittlement (`BRITTLE`) or melted by high heat, once a structure loses connectivity to world anchor (foundation / load-bearing wall) it detaches from static L0–L2 tree into **free voxel rigid island with local L1/L2 subtree (Dynamic Voxel Island)**;
  - Detached rigid island rotates, falls and collides as whole under `RIGID_WELD` constraint, injecting `KINETIC` stimulus back into world grid by momentum on contact (smashing through floors, detonating `VOLATILE` tanks or crushing `GLASS` pipes);
  - World objects, backpack storage slots and biological organ grids share same tag penetration and damage rules.

---

## 4. Prisma engine subsystem modifications and extension design

Following sections define general engine capabilities that need incremental evolution in `crates/prism-*` to fully implement v3.0 sandbox mechanics (all general capabilities must not contain gameplay-specific vocabulary).

### 4.1 `prism-ecs`: dormant active set, multi-rate division and double-buffered parallel Chunk

Current: `prism-ecs` ([`crates/prism-ecs/src/lib.rs`](../../crates/prism-ecs/src/lib.rs)) provides sparse component pool `World`, generational entity handle `Entity`, `World::set_active` active filtering and reusable thread pool [`jobs::scoped_parallel_chunks`](../../crates/prism-ecs/src/jobs.rs) (up to 8 threads).

Engine extension needs:
1. **Chunk / Brick-level active bitmap and wake queue (Dirty Brick Registry)**:
   - Add general `ChunkedGridStorage<T>` / `VoxelTreeStorage` (L1 `u64` active bitmap + L2 `4×4×4` 1 KB brick): when whole L1/L2 is `DORMANT` or full vacuum, `popcnt == 0` skips directly, achieving true **0 CPU usage**.
2. **Four-level deterministic multi-rate scheduler (Multi-Rate Fixed Scheduler)**:
   - On fixed step `60 Hz` provide general division tick token `StepPhase`:
     - **60 Hz Base Step**: `KINETIC` collision response, dynamic voxel island rigid stepping and input sampling;
     - **0–20 Hz Discrete Pulse Bus**: dispatch mechanical, conveyor and circuit pulses by entity / grid `TICK_HZ` phase accumulator;
     - **4–5 Hz Convection / Centrifuge Step**: cross-cell buoyancy exchange, pump-free thermal convection and centrifugal separation (3D fluid macro pressure equalization at L1 nodes, descending to L2 at breaches);
     - **1 Hz Clock Decay Step**: `HALF_LIFE` decay / corruption decrement, `SWITCH_HEAT` natural cooling and `WARNING` countdown.
3. **Multi-color checkerboard parallel stepping (2D 4-color / 3D 8-color Checkerboard)**:
   - Use `jobs::scoped_parallel_chunks` to execute spatial blocks concurrently by `(cx % 2, cy % 2)` (2D 4 batches) or `(cx % 2, cy % 2, cz % 2)` (3D `2×2×2` 8 batches), ensuring no data races across Brick boundary reads / writes and fully deterministic replay.

### 4.2 `prism-reflect` & `prism-scene`: bitflag reflection and RTTIBin zero-compat traversal

Current: `ComponentRegistry` ([`crates/prism-scene/src/registry.rs`](../../crates/prism-scene/src/registry.rs)) projects components to editor, RSCN v4 and `preview-module` C ABI ([`crates/prism-game-api/src/lib.rs`](../../crates/prism-game-api/src/lib.rs)) through `TypeRegistry` and `#[derive(Reflect)]`.

Engine extension needs:
1. **Bitflags reflection descriptor extension**:
   - In `prism-reflect` add field attribute descriptions for wrapped integers with bitflag semantics (`#[reflect(bitflags = "...")]` or bit-name table mapping), letting editor inspector and `prism-editor-api` render checkbox groups instead of raw decimal integers.
2. **Chunk / Cell batch binary component and RTTIBin channel**:
   - In `prism-scene` add general `CellGridChunk` component (fixed-size byte slice + dirty-region version `u64`), ensuring RSCN v4 scene packing and `preview-module` cross-C-ABI transfer only sync changed Chunks, avoiding splitting each cell into tens of thousands of independent `Entity`s.
3. **Static substance ROM data loading contract**:
   - Project-side substance ROM tables driven by pure JSON data in `assets/data/materials.json`, loaded via `GameWorld::asset_exists` / hot reload, not hardcoded into engine.

### 4.3 `prism-physics2d` / `rapier3d` & `prism-engine`: spatial topology core, Teardown break and dynamic voxel islands

Current: `prism-physics2d` ([`crates/prism-physics2d/src/lib.rs`](../../crates/prism-physics2d/src/lib.rs)) has ECS-independent deterministic `Aabb2` and `raycast_aabb`; 3D rigid side has optional feature `prism-app/physics` (`rapier3d`).

Engine extension needs:
1. **Connected-domain and structural-support topology solver (Cavity & Structural Integrity Solver)**:
   - Provide incremental Flood-Fill solver: on one hand input `AIRTIGHT` / `VALVE` masks to partition airtight compartments and pipe network IDs; on other hand execute **local BFS / Flood-Fill connectivity detection with world anchors** in damaged regions, returning detached voxel set on support loss, triggering Teardown-style structural collapse.
2. **Dynamic voxel rigid islands and compound collision bridging (Dynamic Voxel Islands + `RIGID_WELD`)**:
   - Wrap voxel blocks detached from static L0–L2 tree into **dynamic voxel rigid bodies with local L1/L2 subtrees**, compute center of mass and inertia tensor from voxel mass distribution, bridge into `rapier3d` (or `prism-physics2d` in 2D slices) via compound AABB / Convex proxies for free rotation, rolling and collision;
   - Collision solver returns normal impulse `impulse: f32` and contact coordinates, for upper layer to dispatch `KINETIC` stimulus by threshold (driving `BRITTLE` shattering, `PIEZOELECTRIC` discharge, `SHARP` puncture or `VOLATILE` chain detonation on self and target grid, enabling floor smashing and secondary disintegration).
3. **Inter-frame dirty tracking integration**:
   - Integrate `prism-engine::dirty_router::DirtyRouter` ([`crates/prism-engine/src/dirty_router.rs`](../../crates/prism-engine/src/dirty_router.rs)), skipping render instance and GPU brick buffer re-upload when grid region / voxel island visual / lighting state unchanged.

### 4.4 `prism-render` & Slang shaders: transient visual encoding and 3D Compute DDA voxel render

Current: `SpritePass` ([`crates/prism-render/src/sprite_pass.rs`](../../crates/prism-render/src/sprite_pass.rs)) merges all 2D instances into single draw call (`SpriteInstance::flags` currently uses `bit 0`); repo also has 64-tree and Compute pipeline foundation (see [`docs/prt-gi-64tree-design.md`](../../docs/prt-gi-64tree-design.md)).

Engine extension needs:
1. **2D slice fast path (`SpriteInstance::flags` multi-state shader channel)**:
   - Keep `bit 0` as `receive_lights`, open high bits as general visual effect modulation bits, render transient appearances with branchless bit operations in `assets/shaders/slang/sprites.slang`:
     - Surface specular / wet darkening (`WET`), crystalline frost border (`FROZEN` / `FROZEN_LOCKED`), emissive and alarm flashing (`BURNING` / `CHARGED` / `WARNING`), acid corrosion / hypha overlay (`CORRODED` / `INFECTED`), stasis freeze outline (`STASIS`).
   - If modifying `sprites.slang`, synchronously run `bash assets/shaders/compile.sh` and `xtask shader_bindgen` (strictly align `shader_bindings::sprites`).
2. **3D three-level voxel GPU Compute DDA / Raymarching channel (no triangle mesh rebuild)**:
   - Add `VoxelDdaPass` Compute shader: directly step L0 Region → L1 `u64` bitmask (`firstbithigh` hardware skip empty cells) → L2 `4×4×4` 1 KB brick buffer, **completely avoiding CPU/GPU stutter from repeated Greedy Mesh rebuilds under high-frequency Teardown destruction**;
   - Support local inverse-transform ray intersection for dynamic voxel rigid islands (`RIGID_WELD`), natively handling `GLASS` refraction, voxel-gap real-time shadows, fire / arc emissive and smoke volume lighting.
3. **Lighting, particle and thermal debug overlay**:
   - Reuse `Light2D` / 3D lights, `ParticleEmitter2D` (splash debris, boiling steam, acid mist, arc sparks) and `LineRenderer` (laser / arc / `FlowDir` vectors);
   - Extend engine debug view hotkeys: temperature heatmap, pressure isobars, `DORMANT` block wireframes, structural support connected-domain coloring and `INTENSITY (0..15)` gradient.

### 4.5 `prism-audio`: hazard audio-visual alarms and pulse throttling

Current: `prism-app` drives 2D spatialized sound effects and mixing channels through `Subsystem::Audio` and `GameWorld::play_sound`.

Engine extension needs:
1. **High-frequency pulse audio coalescing (Audio Voice Coalescing)**:
   - When `20 Hz` conveyors or hundreds of `PIEZOELECTRIC` cells trigger `KINETIC` simultaneously, merge same timbre by spatial distance within one frame or 50 ms window into max concurrency cap (e.g. ≤ 4 per class), preventing mixer clipping.
2. **`WARNING` critical alarm master control**:
   - Maintain refcount of continuously looping alarm voice channels: any connected domain entering `WARNING` (3–5 s rescue window) raises spatialized alarm sound, immediately switching to depressurization / rupture effect when fault clears or enters `BROKEN`.

### 4.6 `prism-editor-api` & `prism-editor-mcp`: bitmask inspector and causal-chain automated regression

Current: `prism-editor-mcp` exposes document read/write, component add/remove, game run and screenshot tools.

Engine extension needs:
1. **Inspector bitflag grouped rendering**:
   - `prism-editor-api` recognizes `Reflect` bitflag metadata, renders checkbox groups in egui Inspector by three collapsible groups: "material / transient / topology".
2. **MCP headless physical causal-chain probe (Headless Causal Assertion)**:
   - To verify complex emergent chains, later expose deterministic scene load + inject `StimulusFlags` at given coords + step N frames then read `PhysicalCell` assertion in project or MCP test interface, combined with `PRISMA_SCREENSHOT` for render verification.

---

## 5. Engine requirement mapping and acceptance criteria summary table

| Engine module | Driven by which v3.0 mechanism | Core modification | Quantifiable acceptance criteria |
|---|---|---|---|
| `prism-ecs` | `DORMANT`, `TICK_HZ (0..20)`, `HALF_LIFE`, centrifugal `4~5 Hz` | Chunk / brick dormant bitmap, four-level fixed division, 4-color / 8-color checkerboard parallel | 10,000 static voxels CPU time < 0.05 ms; parallel results bit-identical to single-thread |
| `prism-reflect` / `prism-scene` | 15 material / 18 transient / 10 topology bits, C ABI preview mode | Bitflags description projection, `CellGridChunk` / `VoxelBrick` RTTIBin encoding | `preview-module` dynamic load hot reload no schema mismatch, bit panel checkbox-editable |
| `prism-physics2d` / `rapier3d` | `AIRTIGHT`, `CONDUIT`, `RIGID_WELD`, Teardown collapse | Airtight connected domain and structural-support Flood-Fill, dynamic voxel rigid islands, impulse callback | Load-bearing column cut detaches floating island within 1 frame and free-falls; breach `PUNCTURED` immediate depressurization |
| `prism-render` / Slang | 3D destruction render, `WET`/`BURNING`/`FROZEN`/`WARNING`/`STASIS` | 2D `SpriteInstance::flags` + 3D `VoxelDdaPass` Compute stepping | High-frequency destruction zero triangle-mesh rebuild stutter; single-frame visible voxel gaps and transient shading; `xtask shader-budget` green |
| `prism-audio` | `WARNING` (3~5 s window), high-frequency `KINETIC`/`SHOCK` | Spatial audio concurrency throttling and alarm voice lifecycle | 100 impacts on screen no clipping, stable audio-visual alarm 3 s before critical |
| `prism-editor-mcp` | Full causal-chain regression and screenshot gate | Stimulus injection command + `PhysicalCell` state readback | Automated script independently reproduces full chain "cold embrittle → impact shatter → leak conduction → switch meltdown" |

---

## 6. Milestone plan (M0–M5) and risk control

### 6.1 Milestone breakdown

| Milestone | Deliverable | Hard Exit Gate |
|---|---|---|
| **M0: Skeleton and memory core** (scaffolding done) | Establish `projects/emergence/` independent workspace; land 16-byte `PhysicalCell` and 5-dimension enum / bitflag full set with unit tests; complete engine-driven planning doc | `cargo check` / `cargo test` (`preview-module`) and `scripts/check_comments.py` all green; `size_of::<PhysicalCell>() == 16` compile-time assertion passes |
| **M1: Chunked dormancy core and base matter phase transitions** | `prism-ecs` integrates Chunk dormant bitmap and checkerboard parallel; implement temperature-pressure phase diagram (vacuum boiling, high-pressure liquefaction, forced density ladder) and base ROM substance table | Unit tests cover "sealed vessel heating → `WARNING` → breach depressurization → water vacuum boiling"; fully silent grid achieves `DORMANT` zero cost |
| **M2: Electrical / magnetic / thermal / corrosion / piezoelectric closed loop** | Implement `INTENSITY (0..15)` impedance decay, `SWITCH_HEAT` overload meltdown, `WET` short-circuit conduction, `PIEZOELECTRIC` strike generation, `GLASS` acid embrittlement | Build interactive "wet-electric meltdown lab" vertical slice: water flooding insulated floor causes wire short-circuit heating meltdown triggering audio and spark particles |
| **M3: Fluid pipe network, airtight topology and 3D three-level voxels** | Launch L0/L1(`u64`)/L2(`1 KB Brick`) three-level storage and Compute DDA render; connect `CONDUIT` + `VALVE` pressure difference and thermal convection, `FROZEN` pipe blockage rupture, `CONVEYOR` overclocked catapult | Can build closed-loop heat-exchange pipes and 20 Hz catapult launch platform; 3D scene real-time excavation / blast holes zero triangle-mesh rebuild stutter |
| **M4: Teardown structural collapse and bio-nucleus evolution** | Integrate load-bearing connectivity Flood-Fill and `RIGID_WELD` dynamic voxel rigid islands (floor smash secondary detonation); implement `BioActivity` four stages, `BIO_NEST`, `HALF_LIFE` and `STASIS` | Blasting load-bearing column triggers whole building toppling crushing lower tanks; complete "spore revival → parasitism → radiation mutation → stasis lock" full chain |
| **M5: Colored easter eggs, debug views and automated gate** | Complete `sprites.slang` and `VoxelDdaPass` transient shading, multi-channel thermal / support debug views, integrate `PRISMA_SCREENSHOT` and MCP causal-chain test suite | Support desktop standalone binary + editor `preview-module` dual form 60 FPS steady; CI `user-projects` build all green |

### 6.2 Key technical risks and mitigation strategies

| Risk | Trigger | Mitigation |
|---|---|---|
| **High-frequency oscillation causing stutter or lockup** | Redstone gate circuits / valves flipping indefinitely within a second triggering full-grid recompute | Hardware-level `SWITCH_HEAT` fuse and `TICK_HZ <= 20` cap embedded; threshold directly writes `BROKEN` physical open circuit |
| **Airtight and load-bearing Flood-Fill frame-rate jitter** | Full-grid BFS on every state tweak or destruction | Only incrementally recompute connectivity in local bounding box when `AIRTIGHT`/`PUNCTURED`/`VALVE` or support voxels destroyed; other frames O(1) table lookup |
| **Dynamic voxel rigid island count explosion** | Large chain blasts instantly detach hundreds of debris rigid bodies | Isolated debris below threshold size (e.g. `< 8` cells) directly converted to particles / transient debris (`SHARP`) and destroyed, only large structures promoted to `rapier3d` dynamic voxel islands |
| **Editor `preview-module` C ABI data tearing** | Splitting tens of thousands of cells into independent ECS `Entity` causing per-frame RTTIBin serialization timeout | Strictly use `CellGridChunk` (contiguous block byte stream) as cross-boundary sync granularity, with dirty version incremental transfer |
| **Render channel and business coupling polluting engine** | Hardcoding gameplay branches like "poison gas green" / "acid bubbling" in `prism-render` | Shaders only read generalized visual-physics bits (specular wetness, crystalline edges, pulse emissive, jitter flash), concrete colors passed from project-side ROM palette |
| **Multi-platform compile lock and comment gate failure** | Ignoring `Cargo.lock` sync or Chinese comments in Rust source | All project code and engine extensions strictly use English comments, force `check_comments.py` + `cargo clippy -- -D warnings` before commit |
