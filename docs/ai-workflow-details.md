# AI Workflow Details, Multi-Agent Coordination & Test Harnesses

> Extended reference for [`ai-workflow.md`](ai-workflow.md). Read on demand.

## 1. Multi-Agent & Worktree Rules

- **One task = one worktree**: `.worktrees/<YYYYMMDD>-<branch-with-slashes-replaced-by-dashes>`. Never edit the same working tree from two concurrent sessions.
- **Claim before investigating**: check `agent/working`, apply `agent/working`, and comment `🤖 Agent: claiming (<model> / <tool>)` before reading code or logs. On handoff, post `Done / Next / Risks / Verify`.
- **Single writer per issue**: split issues or file ownership when parallelizing.
- **Subagents**: read-only reconnaissance only (max 3 concurrent, non-overlapping paths). All edits, formatting, and git commits stay in the main session.
- **Destructive git is forbidden** without explicit confirmation (`reset --hard`, `checkout -- .`, `clean -fd`, `stash drop`, force-pushing shared branches).
- **No internal jargon in public text**: write self-contained PR/issue prose; use `Part of #N` / `Closes #N` rather than bare `#N`.

## 2. Document Tiers & Temporary Artifacts

- **L0**: `README.md`, `AGENTS.md`, `GEMINI.md` — entry point and hard invariants.
- **L1**: `docs/DESIGN.md`, `docs/REFLECT.md` — architecture and serialization contracts.
- **L2**: `anim-*.md`, `editor-*.md`, `project-module-architecture.md`, `mobile-raytracing-gi-design.md`, `rtx-dlss-integration-design.md`.
- **L3**: `ai-workflow.md`, `ai-workflow-details.md`, `code-style.md`, `versioning.md`, `integrations.md`, `agent-issues.md`.
- **L4–L6**: platform guides (`android-*.md`, `build-guide-termux.md`), diagnostics (`lessons-learned.md`, `rendering-*.md`), showcases.
- New docs must use `kebab-case` with a domain prefix (`[domain]-[topic].md`) and be registered in [`docs/README.md`](README.md).
- Ephemeral scripts belong in `.local/temp/scripts/<category>/` (gitignored); ephemeral plans in `docs/plans/` must be deleted or merged once completed.

## 3. Verification Harnesses (Reuse Instead of Boilerplate)

- **Shaders**: compiled `.spv` and `reflection/*.json` are committed. Rebuild with `bash assets/shaders/compile.sh` and `cargo run -p xtask -- shader-bindgen`.
- **GPU validation**: CI sets `PRISM_REQUIRE_VALIDATION=1` so `VulkanContext` fails instead of silently skipping when `VK_LAYER_KHRONOS_validation` is missing.
- **Compute shader tests**: reuse `GpuHarness` in `crates/prism-render/tests/support/mod.rs` (example: `shader_harness_gpu.rs`). For kernel timing comparisons, use `GpuHarness::dispatch_repeated` with idempotent kernels (`kernel_benchmark_gpu.rs`).
- **Golden frame rendering**: `FrameHarness` in `crates/prism-render/tests/golden_support` runs `GraphRenderer` and stores per-machine self-consistent baselines under `target/golden-baselines/` (`PRISM_UPDATE_GOLDENS=1 cargo test -p prism-render --test golden_frame`). Skips cleanly when no Vulkan driver is present.
- **Editor data layer**: `EditorHarness` in `editor/crates/prism-editor-core/tests/support` (`workspace_harness.rs`).
- **MCP tools**: `McpHarness` in `crates/prism-editor-mcp/tests/support` (`mcp_harness.rs`).
- **egui widgets**: `EguiHarness` in `editor/crates/prism-editor-egui/src/test_harness.rs` (headless `RawInput`, no window/GPU required).
- **Engine simulation**: `SimHarness` in `crates/prism-engine/tests/support` (`sim_harness.rs`, fixed `step` + `Snapshot::diff`). Never advance time with `sleep`.
- **Animation**: `AnimationHarness` in `crates/prism-engine/tests/animation_support` (`animation_harness.rs`).
- **Physics (`--features physics`)**: `PhysicsHarness` in `crates/prism-app/tests/support` (`physics_harness.rs`).
- **Asset pipeline**: `AssetHarness` in `crates/prism-cooker/tests/support` (`asset_harness.rs`).
