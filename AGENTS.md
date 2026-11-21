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

## 6. Reference projects (clean-room)

- "Reference <project>" means read `.local/reference/<project>/` first. If it is not there, shallow
  clone the upstream open-source repository into that path (`git clone --depth 1 <url>
  .local/reference/<project>`); `.local/` is gitignored, so nothing is vendored and no dependency
  is added.
- Take the idea, not the bytes: study the algorithm, data flow, and pipeline shape, then implement
  it independently against this repo's own abstractions. Never copy code, shader source, comments,
  identifiers, or assets from a reference.
- The reference is scaffolding for your own reasoning only. Once you are writing, it does not
  exist: shipped code, comments, commit messages, PR bodies, and review replies must never name the
  reference project, its files, or its authors.

## 7. Git and review

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
- **MANDATORY — ask a human first.** Before running any destructive command, stop and get explicit
  human approval. This is not a judgement call and there is no "it is probably safe" exception.
  Destructive commands include, at minimum: `git reset --hard`, `git reset`, `git checkout -- .`,
  `git checkout <path>`, `git restore .`, `git restore <path>`, `git clean -f`/`-fd`/`-fdx`,
  `git stash drop`/`clear`, `git branch -D`, `git push --force`, `git worktree remove --force`,
  `rm -rf`, `rm` on tracked files, and any truncating or redirecting write over uncommitted work.
  A human approval covers exactly the one command it was requested for; a second destructive
  command needs a second approval. State the exact command, the paths it touches, and what
  uncommitted work it would destroy.
- Uncommitted work is the one thing git cannot recover: `reflog`, `git fsck --lost-found` and IDE
  local history do not hold it, so one mistyped probe destroys it permanently. Diagnose in a
  scratch copy (`cp`) or a separate `git worktree add`; never probe the live worktree. Record
  `git status` and `git rev-parse HEAD` before and after any command that can change history or
  the worktree, and compare.
- A "modified" worktree you cannot explain is a stat-cache or line-ending question, not a licence
  to reset: compare `git hash-object FILE` against `git rev-parse HEAD:FILE` first, and treat a
  match as proof the content is intact. Normalize endings with `python scripts/to_lf.py`, never
  with an ad-hoc `reset`/`checkout`.

## 8. Comments and docs

English only. A comment may state an invariant that code cannot express or link an external spec.
Never restate code, narrate a diff, or leave a `TODO`. `scripts/check_comment_only.py` and
`docs/code-style.md` are the authority. New docs use kebab-case, carry a domain prefix, and are
registered in `docs/README.md`.
