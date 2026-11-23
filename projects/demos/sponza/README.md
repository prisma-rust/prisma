# Prisma Sponza Path Tracing & SHaRC

This is an independent workspace alongside `projects/demos/fishing/` as a user project directory, demonstrating the engine's real-time path-tracing pipeline and NVIDIA RTXGI (SHARC v1.6) global illumination.

## Architecture and design principles

1. **Hard architecture isolation**:
   - Render pipeline changes and SHaRC core implementation all belong to engine layer (`crates/prism-render` / `crates/prism-engine` / `crates/prism-app`), compiled via modular Cargo feature switches. Currently only `sharc` (SHARC) has implementation, `nrc` / `dlss` are reserved switches, see "Implementation scope and unintegrated features".
   - User project (`projects/demos/sponza`) stays clean (UI-framework-free), no third-party UI framework, mounts test tuning UI via engine's public `FrameHook`, only consumes settings and APIs exposed by engine.
2. **Dual build form support**:
   - `player` (default): static player form, links engine, builds desktop test binary or Android JNI library.
   - `preview-module`: editor preview dynamic module form, depends only on cross-ABI pure data layer, doesn't link engine.
3. **Asset scheme (Scheme A)**:
   - Uses lightweight classic Crytek Sponza model (from RTXGI official Assets), loads in seconds, avoids high load triggering GPU device lost.

## Run instructions

> **Don't run `cargo run` directly**: runtime only reads scenes from `assets/scenes.pak`, no disk fallback.
> When package missing window only shows clear color (0.5 gray), log has only one `no resolvable scene`.

One command from repo root (pack scenes + launch):

```bash
bash scripts/dev.sh sponza
```

Below are manual steps.

### 1. Build assets

`scenes.pak` is a generated artifact (not committed), must be packed on first run or after modifying scene documents:

```powershell
# One-key script (under projects/demos/sponza/ directory):
.\rebuild-pak.ps1        # PowerShell
.\rebuild-pak.bat        # CMD / double-click
./rebuild-pak.sh         # Linux / macOS / Git Bash

# Or native Cargo command:
cargo run -q --manifest-path ../../Cargo.toml -p prism-build-pipeline -- build-scenes
```

`build-scenes` locates `assets/` by current directory, so running at repo root packs root assets.

### 2. Launch application

Launch in project directory:

```powershell
cargo run
```

SHaRC now **compiled and enabled by default** (`default = ["player", "sharc"]`). To disable GI at runtime,
no need to change compile options: `SPONZA_RENDER_MODE=pt_off` (see next section).

If you want to remove even compile (`sharc.rs` + resolve pipeline + 3 GPU buffers not in binary), use project's
own `sharc` switch — it's a real switch, disables both `prism-app/sharc` and `prism-engine/sharc`:

```powershell
cargo run --no-default-features --features player
```

### 3. render mode

At startup `SPONZA_RENDER_MODE` selects default mode: `raster` = raster (GI off),
`pt_off` = path tracing without GI, other values or unset = path tracing + SHaRC.

No restart needed at runtime: press `M`, or select `Render Mode` row in panel with `Left` / `Right`
to switch between raster and path tracing (switching resets path-tracing accumulation).

## Test tuning UI (ref RTXGI ImGui)

Tuning panel organized following NVIDIA RTXGI official sample (`Samples/Pathtracer/UI.cpp`), with sections:

### 1. Keyboard shortcuts

| Key | Function |
| :--- | :--- |
| `F1` / `Tab` | Show / hide full tuning panel (compact status bar shown when hidden) |
| `Up` / `Down` | Select parameter row up / down |
| `Left` / `Right` | Decrease / increase selected parameter value |
| `Space` / `Enter` | Toggle bool or execute action |
| `1` / `2` / `3` / `4` | Quick-switch GI mode: `1` = Reference (pure PT), `2` = SHaRC, `3` = NRC (placeholder, no implementation), `4` = ReSTIR GI (raster, reuses same SHaRC cache) |
| `T` | Cycle tonemap operator (Reinhard → ACES → Passthrough) |
| `M` | Switch between raster and path tracing (resets PT accumulation) |
| `R` | Manually trigger PT accumulation cache reset |
| `V` | Toggle VSync |

Window auto-locks mouse on focus (camera takes over keyboard / mouse, `Esc` unlocks; clicking screen also locks, leaving and returning auto-restores lock). **Panel auto-collapses in locked state**; `W` / `S` / `A` / `D` strictly routed by lock state, only one consumer at any time: locked → camera only (panel won't flip even collapsed), unlocked → panel menu only (camera doesn't move). To see parameters press `F1` / `Tab` to bring panel back, navigate menu with `Up` / `Down` / `Left` / `Right`.

`M` / `R` / `V` and `F1` / `Tab` / `1` / `2` / `3` / `4` work in any state — camera only consumes `W` / `A` / `S` / `D` / `Q` / `E` / `Space` / `Ctrl` / `Shift`, these three keys don't affect movement, shouldn't be blocked by lock state.

### 2. Core tuning parameters

- **Render**:
  - `Render Mode`: raster / path tracing switch (`Raster` / `Path Trace`).
  - `Auto Exposure`: histogram auto-exposure toggle (default on; when off tonemap uses fixed exposure,
    consistent with engine default behavior).
- **Path Tracing**:
  - `GI Mode`: global illumination algorithm selection (`Reference` pure Monte Carlo / `SHaRC` spatial hash radiance cache / `ReSTIR GI` raster real-time GI (reuses same SHaRC cache) / `NRC` neural cache, placeholder not integrated). Actually effective: `Reference`, `SHaRC`, `ReSTIR GI`.
  - `Max Bounces`: max path bounce count (1..24, default 4).
  - `Ray Max Dist`: max ray trace distance (100..5000).
  - `Tonemapper`: tonemap mode (ACES / Reinhard / Passthrough).
- **SHaRC (NVIDIA RTXGI v1.6)**:
  - `Enable Clear`: clear hash grid and accumulation buffers each frame.
  - `Enable Update`: allow main ray trace to update world-space radiance cache.
  - `Enable Resolve`: allow 1D compute pipeline to merge temporal samples and perform cache eviction.
  - `Anti-Firefly`: enable specular denoise / firefly suppression filter.
  - `Scene Scale`: controls voxel space size (5.0..100.0, default 50.0).
  - `Roughness Thresh`: roughness threshold (specular / smooth surfaces below threshold skip cache query and trace directly, avoid blurry reflections).
  - `Downscale Factor`: sparse update downsample ratio (default 5, ~4% pixels participate in update).
  - `Accum Frame Num`: temporal accumulation smoothing window frames (1..120, default 20).
  - `Stale Frame Num`: max stale frames before cache slot eviction (1..120, default 60).
- **DLSS** (placeholder, no implementation):
  - `DLSS Mode`: deep learning super-resolution mode (Off / Quality / Balanced / Performance / Ultra Performance / DLAA). Switching only updates enum value, doesn't change render result.

## Implementation scope and unintegrated features

Alignment target is RTXGI v2.9.0. That version split DDGI into separate RTXGI-DDGI repo, mainline is
**world-space radiance cache SHARC**, not probe irradiance volumes. Current integration status:

| Feature | Status | Notes |
| :--- | :--- | :--- |
| Reference pure path tracing | Integrated | Monte Carlo integration, no radiance cache. |
| SHARC (`sharc`) | Integrated | Hash grid + temporal accumulation + `resolve`, wired into path tracing integrator's bounce loop. |
| Histogram auto-exposure | Integrated | Ported from RTXGI `AutoExposure.hlsl`, 256-bin log luminance histogram. |
| DDGI | Not integrated | Not in RTXGI v2.9.0; engine-side probe representation kept in `crates/prism-render/src/gi.rs`, but no baked data load path yet. |
| NRC (`nrc`) | Not integrated | NVIDIA Neural Radiance Cache is closed-source Tensor Core binary SDK, neither bundled nor redistributable, no linkable implementation. |
| DLSS / Streamline / NGX (`dlss`) | Not integrated | Requires NVIDIA Streamline SDK and driver-side distribution license, not in dependency tree; no denoiser or upscaler in repo either. |

Therefore:

- `nrc` / `dlss` Cargo features currently empty switches, no `#[cfg(feature = "nrc")]` / `#[cfg(feature = "dlss")]` implementations; `dlss_mode` has no read point in render pipeline either.
- Tuning UI's `NRC (Neural)`, `DLSS Mode` items and shortcut `3` are enum placeholders, switching won't change render output.
- NVIDIA reference images went through NRD denoising and DLSS Ray Reconstruction, this repo outputs raw un-denoised accumulation, noise and luminance distribution can't be compared pixel-by-pixel.
