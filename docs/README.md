# Prisma Documentation System and Directory Specification

This document is the **overview map** and **documentation engineering specification** for the `docs/` directory, defining repository-wide documentation content granularity standards, directory structure conventions, file naming formats, and knowledge maintenance principles.

---

## 🧭 1. Core Maintenance Principles

1. **Long-term validity principle**: `docs/` only archives documents with long-term architectural guidance value, design contracts, and engineering specifications. Session-temporary plans, implementation checklists, and temporary notes are handled per [`ai-workflow.md`](ai-workflow.md) §9: they may be temporarily placed in `docs/plans/`, but **deleted once done**; valuable conclusions are merged into the corresponding authoritative document, and the plan files themselves are not retained.
2. **Zero-compatibility logic before 1.0**: The current version is `0.1`. Before the official 1.0 release, **writing any compatibility logic is strictly prohibited; backward compatibility is explicitly regarded as redundant garbage code**. Documents must not retain maintenance notes or compatibility fallback suggestions for deprecated transitional features; all architecture descriptions follow the latest and best implementation.
3. **Single Source of Truth**:
   - Global architecture and rendering intent follow [`DESIGN.md`](DESIGN.md);
   - Runtime reflection and binary protocol follow [`REFLECT.md`](REFLECT.md);
   - Code map, matrix coordinates, and hard constraints follow [`../AGENTS.md`](../AGENTS.md);
   - Collaboration workflow and engineering gates follow [`ai-workflow.md`](ai-workflow.md).
   Code and documents are updated synchronously in **the same PR**; document drift is rejected.

---

## 📐 2. Content Granularity Standards

Repository documents are divided into 7 levels by content granularity and lifecycle; each level has clear responsibility boundaries, prohibiting cross-level stacking or content granularity imbalance:

```text
L0: Repository portal and hard constraint layer (README / AGENTS / CONTRIBUTING)
      │
L1: Core architecture and protocol foundation layer (DESIGN / REFLECT)
      │
L2: Specialized subsystem design contract layer (anim-* / editor-* / module / raytracing)
      │
L3: Collaboration workflow and engineering governance layer (ai-workflow / code-style / versioning / ...)
      │
L4: Platform access and build/ops layer (android-* / termux / slang)
      │
L5: Diagnostic methods and troubleshooting knowledge layer (lessons-learned / rendering-investigation)
      │
L6: Research inspiration and showcase material layer (shadertoy / showcase)
```

| Level | Responsibilities and content granularity | Admissible content | Prohibited and eliminated content |
|---|---|---|---|
| **L0 Repository portal** | Project facade, core features, quick start, technical hard rules, and code map | Project positioning, architectural highlights, quick run commands, untouchable red lines | Local function implementation details, temporary debug logs, outdated architecture remnants |
| **L1 Core architecture** | Engine global system topology, Render Graph rendering pipeline, reflection and binary layout foundation | Architecture principles, Pass topology, memory model, RSCN/RTTIBin specification | Single bug fix ledgers, transitional temporary compatibility solutions |
| **L2 Subsystem contract** | Module-level specifications, mathematical models, state transitions, and ABI/API contracts for each functional subsystem | Animation graph/IK/retargeting math, editor data layer, project module C ABI | Historical design drafts conflicting with the latest implementation, unimplemented generalization fantasies |
| **L3 Process governance** | AI/human collaboration workflows, code and comment style, version evolution mechanisms, gate rules | Five hard rules, Commit/PR templates, self-check gates, DoD | Arbitrarily changed personal verbal agreements, non-automated slogan-like norms |
| **L4 Platform ops** | Platform compilation, cross-compilation toolchains, packaging and release operation guides | Android NDK, Termux build commands, Slang shader compilation workflow | Old environment patches without reproduction value, expired system configurations |
| **L5 Troubleshooting knowledge** | Deep debugging troubleshooting decision trees, Vulkan synchronization and lifecycle root cause analysis | Binary-search diagnosis methodology, typical VUID root cause analysis, lessons never to repeat | Surface error memos without root cause analysis, scattered debug snippets |
| **L6 Showcase inspiration** | Audiovisual showcase scripts, rendering algorithms, and shader inspiration notes | Showcase video visual and audio parameters, Shadertoy porting algorithm ideas | Expired copy containing abandoned architecture descriptions |

---

## 🗂️ 3. Directory Structure and File Naming Conventions

### 1. Unified File Naming Standard

To ensure cross-operating system (Windows / Linux / macOS) file system compatibility and the robustness of scripts/compile-time macros, all document names follow these iron rules:

1. **Technical topics and process documents are all lowercase kebab-case**: `kebab-case.md`.
2. **Prefix-clustered semantic structure**: The format is uniformly `[domain]-[topic][-detail].md`. Documents of the same subsystem must share a domain prefix:
   - Animation subsystem: `anim-*.md`
   - Editor subsystem: `editor-*.md`
   - Platform and environment: `android-*.md`
3. **Core foundational documents retain all-uppercase**:
   - `README.md`: Directory entry index and panorama map.
   - `DESIGN.md`: Highest authoritative source of engine architecture design intent.
   - `REFLECT.md`: Highest authoritative source of reflection system and binary format.
4. **Naming anti-patterns**: Strictly prohibit camelCase (`AnimGraph.md`), underscores (`ai_workflow.md`), or prefix-less fragmentary naming without a domain prefix.

### 2. Directory Organization Topology

Documents physically use a **prefix-clustered flat directory**, and logically organize into 6 major domains. The flat prefix-clustered structure balances file browser natural ordering with cross-module relative reference safety (e.g., Rust `include_str!` compile-time macro directly binds `docs/editor-mcp.md`):

```text
docs/
  ├── README.md                           # This document (documentation system map and specification)
  ├── DESIGN.md                           # [L1 Core architecture] Engine and rendering overall architecture design source
  ├── DESIGN.zh-CN.md                     # [L1 Core architecture] DESIGN.md Chinese reference version (authoritative version is English)
  ├── REFLECT.md                          # [L1 Core architecture] Reflection system and self-describing binary specification
  │
  ├── anim-graph.md                       # [L2 Animation subsystem] Animation graph architecture and .rag specification
  ├── anim-ik.md                          # [L2 Animation subsystem] Analytical two-bone IK and ground contact
  ├── anim-motion-matching.md             # [L2 Animation subsystem] Motion matching pure math layer and feature database
  ├── anim-retarget.md                    # [L2 Animation subsystem] Skeletal pose retargeting pure math layer
  ├── editor-egui.md                      # [L2 Editor subsystem] Native egui editor architecture and zero-copy sampling
  ├── editor-mcp.md                       # [L2 Editor subsystem] MCP automation contract and tool registry
  ├── editor-self-hosted-ui.md            # [L2 Editor subsystem] Self-hosted editor and unified engine UI API architecture design
  ├── editor-ui-requirements.md           # [L2 Editor subsystem] New editor UI feature requirement contract (numbered requirements + phased acceptance)
  ├── scripting-layer.md                  # [L2 Module host] Scripting layer seam: HostApi reuse, Lua/C# phasing and criteria
  ├── feature-map.json                    # [L2 Editor subsystem] Editor UI node tree and interaction feature map
  ├── project-module-architecture.md      # [L2 Module host] Game project C ABI and dynamic preview module architecture
  ├── mobile-raytracing-gi-design.md      # [L2 Rendering advanced] Mobile RayQuery and real-time GI pipeline blueprint
  ├── rtx-dlss-integration-design.md      # [L2 Rendering advanced] DLSS / ray reconstruction / frame generation guide buffer contract and milestones
  ├── prt-gi-64tree-design.md             # [L2 Rendering advanced] PRT64 precomputed radiance transfer, 64-ary tree streaming and M1–M4 acceptance contracts
  ├── src-gi-design.md                    # [L2 Rendering advanced] SRC GI (surfel radiance cascade) mobile dynamic GI alternative design blueprint
  ├── voxel-software-gi-design.md         # [L2 Rendering advanced] Voxel-specific software GI (64-ary tree bitmask DDA + face cache + 3D radiance cascade)
  ├── voxel-rtx-gi-design.md              # [L2 Rendering advanced] Voxel-specific hardware RTX GI (RT Core + face cache + ReSTIR GI)
  ├── virtual-geometry-design.md          # [L2 Rendering advanced] Multi-platform virtual geometry (Beyond-Nanite Cluster-DAG / software-hardware hybrid rasterization / ray tracing same source)
  │
  ├── ai-workflow.md                      # [L3 Collaboration workflow] AI collaboration workflow and five hard rules (concise quick reference)
  ├── ai-workflow-details.md              # [L3 Collaboration workflow] Multi-Agent collaboration and test harness details
  ├── code-style.md                       # [L3 Coding standard] English code comment standard and redundancy cleanup guide
  ├── naming-and-abstraction-guide.md     # [L3 Coding standard] Code and shader naming and content abstraction specification
  ├── versioning.md                       # [L3 Version governance] SemVer rules, zero-compatibility logic before 1.0 and releases
  ├── integrations.md                     # [L3 Engineering governance] CI/CD, Discord bot and Agent claiming contract
  ├── agent-issues.md                     # [L3 Specification gate] Agent issue raising criteria and deduplication mechanism
  ├── agents-contract-audit.md            # [L3 Specification gate] External Agent contract template audit snapshot against repository status
  │
  ├── android-apk.md                      # [L4 Platform ops] Android game-only standalone APK build and packaging
  ├── android-arm64.md                    # [L4 Platform ops] aarch64 host Android toolchain replacement
  ├── android-chroot-turnip-gpu.md        # [L4 Platform ops] Sidebar: Android chroot container direct Adreno GPU (Turnip + KGSL)
  ├── build-guide-termux.md               # [L4 Platform ops] Termux / Android native terminal compilation guide
  ├── slang-bindless-migration.md         # [L4 Platform ops] Slang shader toolchain and Bindless resource maintenance
  │
  ├── lessons-learned.md                  # [L5 Experience accumulation] Vulkan rendering loop and engineering pitfall root cause library
  ├── rendering-pipeline-investigation.md # [L5 Diagnostic method] Rendering exception troubleshooting decision tree and binary-search diagnosis methodology
  ├── rendering-debug-isolation.md        # [L5 Diagnostic method] Rendering exception four-dimensional isolation analysis (Pass/material/sync/time domain) and operation guide
  │
  ├── shadertoy-inspirations.md           # [L6 Research inspiration] Common graphics effects and Shadertoy algorithm notes
  └── showcase-video.md                   # [L6 Showcase material] Engine automated showcase video technical script and parameters
```

---

## 🗺️ 4. Panorama Document Quick Reference Index

| Category / domain | Recommended entry | Core focus | Associated code / module |
|---|---|---|---|
| **Architecture overview** | [`DESIGN.md`](DESIGN.md) | Multi-end unified, experimental high-performance rendering pipeline, Render Graph topology, platform bricks and hot plugging, zero compatibility baggage; Chinese reference see [`DESIGN.zh-CN.md`](DESIGN.zh-CN.md) | `prism-render`, `prism-engine` |
| **Reflection and data** | [`REFLECT.md`](REFLECT.md) | `#[derive(Reflect)]`, RTTIBin, RSCN scene serialization | `prism-reflect`, `prism-scene` |
| **Animation system** | [`anim-graph.md`](anim-graph.md) | State machine, clip mixing, hierarchical mixing nodes | `prism-engine::anim` |
| | [`anim-ik.md`](anim-ik.md) | Analytical two-bone IK, Foot Placement wiring | `crates/prism-anim-ik` |
| | [`anim-retarget.md`](anim-retarget.md) | Cross-skeleton rotation mapping, bone length proportion scaling | `crates/prism-anim-retarget` |
| | [`anim-motion-matching.md`](anim-motion-matching.md) | Trajectory sampling, feature nearest neighbor search, foot locking | `crates/prism-motion-matching` |
| **Editor ecosystem** | [`editor-egui.md`](editor-egui.md) | Zero-copy offscreen sampling, Split-tree layout, control channel | `editor/`, `prism-editor-core` |
| | [`editor-mcp.md`](editor-mcp.md) | MCP toolset, scene review, offscreen screenshot, automation drive | `crates/prism-editor-mcp` |
| | [`editor-self-hosted-ui.md`](editor-self-hosted-ui.md) | Self-hosted editor layering, ECS UI authoritative ownership, capability checklist and phased delivery | `projects/editor`, `prism-scene::ui` |
| | [`editor-ui-requirements.md`](editor-ui-requirements.md) | New editor feature requirement contract, inspector field specification, UI capability metrics and phased acceptance | `projects/editor`, `docs/editor-ui-requirements.md` |
| | [`project-module-architecture.md`](project-module-architecture.md) | Preview module cdylib, C ABI contract, HostApi table | `prism-project-api`, `prism-runtime-host` |
| | [`scripting-layer.md`](scripting-layer.md) | Script layer HostApi reuse, dynamic component access gap, Lua/C# phasing criteria | `prism-game-api`, `prism-reflect` |
| **Rendering topics** | [`mobile-raytracing-gi-design.md`](mobile-raytracing-gi-design.md) | RayQuery inline, SHARC radiance cache | `prism-render` |
| | [`rtx-dlss-integration-design.md`](rtx-dlss-integration-design.md) | DLSS-SR/RR/G, guide buffer handle and format, degradation ladder | `prism-render`, `assets/shaders/` |
| | [`prt-gi-64tree-design.md`](prt-gi-64tree-design.md) | PRT64 data contract, Pass 1–4 topology, anti-light-leak, M1–M4 acceptance criteria | `prism-render`, `prism-build-pipeline` |
| | [`src-gi-design.md`](src-gi-design.md) | SRC GI (Surfel software ray tracing + 3D radiance cascade) mobile UGC alternative blueprint | `prism-render` (planned) |
| | [`voxel-software-gi-design.md`](voxel-software-gi-design.md) | Voxel software GI (64-ary tree bitmask DDA + face cache + 3D radiance cascade) | `prism-render` (planned) |
| | [`voxel-rtx-gi-design.md`](voxel-rtx-gi-design.md) | Voxel hardware RTX GI (RT Core + face cache + ReSTIR GI / DLSS-RR) | `prism-render` (planned) |
| | [`virtual-geometry-design.md`](virtual-geometry-design.md) | Multi-platform virtual geometry (Cluster-DAG / software-hardware hybrid raster / CLAS ray tracing same source / DMM / SGGX) | `prism-asset`, `prism-cooker`, `prism-render` |
| | [`slang-bindless-migration.md`](slang-bindless-migration.md) | Slang compilation, binding code generation, Bindless texture table | `assets/shaders/`, `xtask` |
| **Collaboration norms** | [`ai-workflow.md`](ai-workflow.md) | Five hard rules, atomic commit, self-check gate, quick commands | Entire repository |
| | [`ai-workflow-details.md`](ai-workflow-details.md) | Multi-Agent collaboration, document layering, test harness details | Entire repository |
| | [`code-style.md`](code-style.md) | Pure English comments, delete code repetition, delete old-version compatibility shims | Entire repository `.rs` source |
| | [`naming-and-abstraction-guide.md`](naming-and-abstraction-guide.md) | Code and shader naming and content abstraction specification (disambiguation, de-colloquialization, zero compatibility) | Entire repository source and shaders |
| | [`versioning.md`](versioning.md) | 0.1 evolution strategy, single version source, format version upgrade | `Cargo.toml`, `scripts/version.py` |
| | [`integrations.md`](integrations.md) | CI/CD tag gate, Issue automation claiming | `.github/workflows/` |
| | [`agent-issues.md`](agent-issues.md) | Deduplication criteria, reproduction template, prohibit false compatibility regression | GitHub Issues |
| | [`agents-contract-audit.md`](agents-contract-audit.md) | External contract template line-by-line comparison, existing measurement data, pending decision items | `AGENTS.md`, `scripts/` |
| **Platform practice** | [`android-apk.md`](android-apk.md) | Gradle host, GameActivity, product slimming | `projects/template/android/` |
| | [`android-arm64.md`](android-arm64.md) | aarch64 host standalone toolchain and NDK replacement | `scripts/build-android-game.sh` |
| | [`build-guide-termux.md`](build-guide-termux.md) | Mobile terminal native compilation, static check verification exception | Termux terminal environment |
| **Troubleshooting** | [`lessons-learned.md`](lessons-learned.md) | Vulkan synchronization, Framebuffer ordering, semaphore reuse | `docs/lessons-learned.md` |
| | [`rendering-pipeline-investigation.md`](rendering-pipeline-investigation.md) | Rendering anomaly binary search, black screen troubleshooting, environment decoupling | `crates/prism-render/tests/` |
| | [`rendering-debug-isolation.md`](rendering-debug-isolation.md) | Four-dimensional orthogonal isolation (Pass bypass / material override / pessimistic barrier / time freeze) SOP | `crates/prism-render/src/render_debug.rs` |
| **Showcase inspiration** | [`shadertoy-inspirations.md`](shadertoy-inspirations.md) | Noise, volumetric clouds, shader inspiration formulas | `assets/shaders/` |
| | [`showcase-video.md`](showcase-video.md) | Automated video synthesis script, motion and audio specifications | `scripts/showcase_video_simple.py` |

---

## 🛠️ 5. New and Modified Document Checklist

When writing or updating any document, the following self-checks must be performed:

- [ ] **Granularity alignment**: Does the content match the granularity positioning of its level? No temporary session plans or local ledgers written into long-term documents.
- [ ] **Naming compliance**: Does the new document name follow the `kebab-case.md` specification and include the correct domain prefix?
- [ ] **Zero compatibility garbage**: Have all forward/backward compatibility redundant descriptions been thoroughly cleared? When format or API changes are involved, state the latest design directly.
- [ ] **Map registration**: Is the new document registered in this file's "directory organization topology" and "panorama quick reference index"?
- [ ] **Cross-verification**: Do the file paths and line numbers referenced in the document truly exist and are valid on the current branch?
