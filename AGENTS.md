# Prisma — Agent Contract

Rust game engine: Vulkan, Android + desktop, data-oriented ECS.

This file holds only rules that compilers, linters, and scripts cannot enforce. Everything else
belongs to tooling. Code map: `README.md`. Subsystem specs and doc levels: `docs/README.md`.
Process: `docs/ai-workflow.md`. Read `docs/DESIGN.md` before touching rendering or architecture.

## 1. Verification gate (non-negotiable)

- Do not call a task done, commit for `master`, or open a PR until `python scripts/pr_ready.py`
  exits 0. `--quick` (fmt + clippy) is for the inner loop; `--all` is required after dependency,
  toolchain, or workspace-level edits.
- Paste the Markdown block `pr_ready` prints into the PR. Where behavior is observable, attach
  runtime evidence: headless logs, state snapshots, `PRISMA_BENCH_REPORT` JSON, or
  `PRISMA_SCREENSHOT` PNGs. A diff alone is not evidence.
- When a check fails, fix the invariant it names. Never edit the check, add `#[allow(...)]`, skip
  a test, or relax a budget to get green.
- Termux / aarch64 only: static checks (`cargo fmt`, `cargo clippy`) may replace the full gate, and
  the reply must list what was not verified. The full gate resumes on any other host.
- `.githooks/pre-push` runs the same gate once installed with
  `git config core.hooksPath .githooks` (that path also enforces commit subjects). Do not bypass
  either hook with `--no-verify`.

## 2. Root-cause discipline

- Never mask a defect with retries, timers, sleeps, polling, or catch-all fallbacks. Fix the state
  machine, the owner of the state, or the abstraction.
- One boolean, one truth. Ownership flags (input capture, pointer lock) have a single writer in the
  platform layer; consumers re-read every frame instead of caching snapshots.
- Never widen a tolerance or disable validation layers to make a timing or state bug disappear.
- If a requested spec (algorithm, format, pipeline) is blocked by the environment, stop and report
  the blocker. Never substitute a cheaper approximation without asking.

## 3. Boundaries (do not regress)

- The player chain stays UI-free and editor-free:
  `template → prism-app → prism-engine → {scene, ecs, render, asset, physics2d}`.
  `cargo tree | grep -i egui` must be empty; `python scripts/check_deps.py` is the gate.
- Optional capabilities stay default-off: `prism-app/{physics,audio}`, `prism-asset/async-io`.
- One implementation per concern: asset paths only via `prism_engine::asset_paths` (no cwd-relative
  paths), `settings.ini` only via `prism-ini`, scene manifests only via the manifest API.
- Zero compatibility burden before 1.0 (currently 0.1). Replace and delete old designs; never add
  shims, deprecated aliases, or version fallback branches.
- Preview modules cross the C ABI as plain operations plus RTTIBin bytes. Never pass `Engine`,
  `World`, or trait objects across it.
- Touch only the code path that produces the bug. No opportunistic refactors, no `git blame` or
  third-party source dives to confirm what the code already states.
- Subagents are read-only reconnaissance (max 3 concurrent, non-overlapping). All writes and
  commits stay in the main session.

## 4. Conventions that silently corrupt pixels

Right-handed, camera looks down −Z, +Y up, +Z toward the viewer. `mat4` is column-major
`[[f32; 4]; 4]` indexed `[col][row]`; `clip = projection * view * model`. Projection uses the
Vulkan y-flip `p[1][1] = -inv_tan(fovy / 2)` with depth `[0, 1]`; NDC y is −1 at the top, +1 at
the bottom. `GraphFrame::inv_view_rot` is the transpose of `view`'s upper-left 3×3, not the
forward matrix. Deviating from any of this is a bug, not a style choice.

## 5. Assets, shaders, scenes

- Slang sources live in `assets/shaders/slang/`; compiled `.spv` and `reflection/*.json` are
  committed and `slangc` is pinned. `assets/shaders/compile-stamp.json` records the version of the
  sources the bytecode was built from, so builds only recompile when a `.slang` file actually
  changes. Push-constant structs come from `xtask shader_bindgen` and are never hand-written.
- `.scene.json` is the only authored scene form: `transform` is baked into `LocalTransform` +
  `LocalToWorld`, matrix propagation runs at the end of the simulation step, and entity `id` is
  stable across edits.
- Runtime scenes load from `assets/scenes.pak`; rebuild it with
  `cargo run -p prism-build-pipeline -- build-scenes` after editing scene documents.

## 6. Git and review

- `master`: one task = one branch = one PR, default diff under ~500 lines. `dev`: direct commits
  and pushes are allowed, still atomic and still gated. `dev → master` goes through a PR.
- Before pushing: `git fetch origin && git rebase origin/<base>`, resolve locally, re-run the gate,
  then `--force-with-lease` only.
- Conventional Commits, short: `<type>(<scope>): <imperative summary>`. Body only when the reason
  is not obvious; verification dumps and per-file narration belong in the PR, not the commit.
- Declare AI output: tool and model on the first line of PRs and issues, `🤖 Agent:` prefix on
  comments. Never impersonate a human.
- Stop and ask before changing public APIs, save formats (RSCN / RTTIBin / `.pak`), coordinate
  conventions, workspace layering, or adding dependencies.

## 7. Comments and docs

English only. A comment may state an invariant that code cannot express or link an external spec.
Never restate code, narrate a diff, or leave a `TODO`. `scripts/check_comment_only.py` and
`docs/code-style.md` are the authority. New docs use kebab-case, carry a domain prefix, and are
registered in `docs/README.md`.
