# Code and shader naming and content abstraction conventions

This document is the content abstraction and naming contract guide for **Rust source code and Slang shaders** in the Prisma codebase.

Core principle: **File name equals abstraction responsibility. Reject misnamed files, reject abuse of pattern nouns, eliminate graphics abbreviation conflicts, and firmly implement zero-compatibility garbage code principle before 1.0.**

---

## 🎨 I. Shader system abstraction and naming conventions

Shaders are located in `assets/shaders/`, source code uses Slang language (`slang/`), compiled to SPIR-V (`.spv`) via `compile.sh`, and strong-type Rust bindings exported via `xtask shader-bindgen` (`crates/prism-render/src/shader_bindings/`).

### 1. File role binary division

Shader files are strictly divided into two categories based on content abstraction, mixed definition is prohibited:

| Category | Role definition | Content | Output form | Naming convention |
|----------|----------------|---------|-------------|-------------------|
| **Entry Shaders** (pipeline entry shaders) | Correspond to a specific RenderPass or Compute task | Contains `[shader("vertex")]`, `[shader("fragment")]` or `[shader("compute")]` | Output `<name>.<stage>.spv` and `reflection/<name>.json` | `<pass_name>.slang` (e.g. `gizmo.slang`, `post.slang`) |
| **Module Libraries** (shared algorithm and structure libraries) | Pure algorithm evaluation, data structure definition, or utility functions | `struct`, public constants, `import` modules, **no entry points** | **No independent .spv generated**, only imported by other shaders | `<concept>.slang` (e.g. `common.slang`, `probe_volume.slang`) |

### 2. Graphics abbreviation disambiguation iron law

In the rendering pipeline, multiple different technical fields often share similar abbreviations, must be completely disambiguated in abstract naming:

| Technical field | Convention prefix / abbreviation | Strictly prohibited ambiguous writing | Reason and boundary |
|-----------------|----------------------------------|--------------------------------------|---------------------|
| **Ray / Path Tracing** | `pt_` / `ray_` | Strictly prohibit abbreviation as `rt_` | `pt_render.slang`, `pt_reference.slang`. `pt_` specifically refers to path integration and ray query. |
| **Render Texture** | `rtt_` / `render_texture_` | Strictly prohibit abbreviation as `rt_` | Historically `rt_render` / `rt_preview` easily misunderstood as Ray Tracing. Unified use `rtt_` prefix for offscreen texture production and consumption testing. |
| **Probe-Volume SH GI** | `probe_` / `probe_volume_` | Strictly prohibit generic naming as `gi_` | Probe volume evaluation library (SH Radiance / Irradiance) only handles regular grid probes, naming must be specific to `probe_volume`, must not impersonate overall GI. |
| **Screen-Space Ambient Occlusion** | `gtao` | Keep lowercase industry standard name | Corresponds to Ground Truth Ambient Occlusion. |
| **Temporal Anti-Aliasing** | `taa` | Keep lowercase industry standard name | Corresponds to TAA Resolve Pass. |

### 3. Pipeline stage suffix and duplicate naming anti-pattern

- **Standard pattern**: single Pass vertex and fragment entry points combined in one `<pass_name>.slang`, compiled by `compile.sh` separately into `<pass_name>.vert.spv` and `<pass_name>.frag.spv`.
  - Positive example: `post.slang` → `post.vert.spv` + `post.frag.spv`
  - Positive example: `gtao.slang` → `gtao.vert.spv` + `gtao.frag.spv`
  - Positive example: `shadow_depth.slang` → `shadow_depth.vert.spv` + `shadow_depth.frag.spv`
- **Anti-pattern**: hardcode stage name in base filename, causing duplicate redundant physical suffixes.
  - Historical anti-pattern: `mesh_vert.slang` → `mesh_vert.vert.spv` (`vert` repeated twice)
  - Historical anti-pattern: `scene_frag.slang` → `scene_frag.frag.spv` (`frag` repeated twice)
  - **Normalization evolution path**: forward main rendering pipeline abstracted as `pbr_forward.slang`, sharing unified `PbrPush` constant block, outputting `pbr_forward.vert.spv` and `pbr_forward.frag.spv`.

---

## 🦀 II. Rust code system abstraction and naming conventions

### 1. Domain concept-driven vs design pattern nouns

In system and engine layer code, module and file names should directly express the **domain model and physical/logical entities** they represent, strictly prohibit using empty design pattern nouns to pollute abstraction:

- **Reject design pattern suffixes**:
  - ❌ `input_facade.rs` (using design pattern Facade as business core name)
  - ✅ **`input_snapshot.rs`** (accurate abstraction: immutable snapshot of input state for one frame, data entity passed across modules)
- **Reject colloquial slang, use precise technical abstraction nouns**:
  - ❌ `dummy.rs` (test double colloquial, blurs true GPU resource meaning)
  - ✅ **`placeholder.rs`** (accurate abstraction: 1x1 constant samplable resource `SolidImage` filling descriptor when feature is off)
- **Reject vague `compat`, precisely identify architecture intent**:
  - ❌ `vulkan_compat.rs`
  - ✅ **`device_fallback.rs`** (accurate abstraction: instruction-level degradation scheduling dispatch when hardware does not support certain advanced Vulkan extensions)

### 2. Zero-compatibility logic and naming cleanliness before 1.0

Before 1.0 release, firmly implement the hard rule: **"old version compatibility is explicitly regarded as redundant garbage code"**:

1. **Strictly prohibit introducing and retaining functions or interfaces with `legacy_` prefix**:
   - Completely remove `legacy_settings_path`, `legacy_manifest_path`, `parse_legacy_toml` and other fallback logic;
   - Configuration file unique source is `assets/settings.ini`, scene manifest unique source is `assets/scenes.json`. Read end and write end only recognize latest format, no writing fallback compatibility branches.
2. **Strictly prohibit retaining `#[deprecated]` shims in public API or internal modules**:
   - When needing to modify signature, data structure, or protocol, directly perform destructive refactoring with optimal design, clean up all old implementations, leaving no transitional compatibility baggage.

---

## 📋 III. Naming and abstraction review checklist

When adding or refactoring code, shader files, must check the following dimensions:

- [ ] **Semantic authenticity**: Does the file name precisely describe the core data structures and algorithms inside? Is there any borrowed big word (e.g. calling local offline tracing `gi_render`)?
- [ ] **Abbreviation exclusivity**: If involving graphics abbreviations, is the disambiguation rule strictly followed (`pt_` ray tracing vs `rtt_` offscreen texture)?
- [ ] **Symmetry and stage decoupling**: Is shader driven by Pass logic name, avoiding appending `_vert` / `_frag` in source main name?
- [ ] **No design pattern noun pollution**: Does module name directly reflect domain concept (e.g. `input_view`), rather than design pattern noun (`input_facade`)?
- [ ] **No colloquialism and slang**: Does underlying resource use formal technical entity name (e.g. `placeholder`), prohibiting `dummy`, `temp`, `fake`?
- [ ] **Zero compatibility garbage**: Is there no `legacy_*`, `*_compat`, or old format fallback parsing code in code?
