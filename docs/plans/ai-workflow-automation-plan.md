# Prisma AI Workflow Automation Optimization Plan (Tiered Verification, Change Gate, Budget Governance)

> **Tier**: L3 process governance · Implementation plan (landed, then merged into [`ai-workflow.md`](../ai-workflow.md) §11 and [`integrations.md`](../integrations.md))
> **Status**: Reviewed and optimized, landed per M1–M4 in separate PRs
> **Date**: 2026-11-19
> **Author**: JasonGu's AI agent (DeepSeek V4.1 Flash / OpenCode)
> **Related**: [`ai-workflow.md`](../ai-workflow.md), [`integrations.md`](../integrations.md), [`agent-issues.md`](../agent-issues.md), `scripts/pr_ready.py`, `.github/workflows/ci.yml`

This document defines the optimization plan for Prisma's existing AI collaboration pipeline: change "full verification every round" to tiered verification converging on change surface, restrict periodic automation to run **only when there are new commits**, and plug the only "trust instead of verify" gap in the current gate. Goals: reduce token, wall-clock, and human review attention; not sharding the repo or jobs.

---

## 1. Goals, Scope, and Definition of Done

### 1.1 Goals

1. **Trustworthy gate**: the self-check block in a PR must bind to current HEAD; expired or forged blocks cannot pass.
2. **Tiered verification**: agent local, PR, periodic, release four layers each run only the checks needed for that layer; expensive checks do not enter the high-frequency loop.
3. **Periodic automation runs only on changes**: no new commits → ends at guard, all downstream skipped.
4. **Outer loop zero blast radius**: start with read-only patrol; code-writing worker default off, allowlist-based.

### 1.2 Non-goals

- Do not shard repo, do not shard crate, do not add workspace.
- Do not introduce automerge.
- Do not run 7×24 resident agent process.
- Do not make fuzzing auto-open issue self-healing loop.
- Do not shard CI into more jobs for parallelism.

### 1.3 Budget premise

`prisma-rust/prisma` is a public repo; GitHub-hosted runner Actions minutes are not billed. Therefore "budget balance" is not CI minutes, but:

| Budget | Consumption method | Corresponding measure in this document |
|--------|-------------------|----------------------------------------|
| LLM token | Each round reading long docs, round trips after compile failure | §8, §5.2, §4 quick-first |
| Wall clock | Each job's fixed overhead and repeated compilation | §7 |
| Human review attention | Re-checking self-check blocks, judging whether green is trustworthy | §5.1 |

If switching to self-hosted runner later, §7's benefit changes from "wall clock" to "real money", priority rises accordingly.

### 1.4 Definition of done

| Milestone | Content | Acceptance |
|-----------|---------|------------|
| M1 | Self-check block binds HEAD | Same HEAD repeated paste passes; old HEAD block rejected; `needs/self-check` weekly trigger trends to 0 |
| M2 | Change gate + read-only patrol | Days with no new commits, workflow ends at `guard`, all downstream `skipped`; when new commits, runs only once |
| M3 | Doc and context layering | Each `area/*` has clear minimum must-read set; `ai-workflow.md` core part <100 lines |
| M4 | Worker (default off) | Switch on → single issue produces only one PR; no duplicate PRs; no direct `dev` push |

---

## 2. Status and Evidence

The following conclusions are all directly locatable in repo files; "conclusion strength" distinguishes confirmed facts from pending inferences.

| Observation | Evidence | Conclusion strength |
|-------------|----------|---------------------|
| Repo is public, Actions minutes not billed | `api.github.com/repos/prisma-rust/prisma` → `"private": false` | Confirmed |
| Compiled SPIR-V and reflection JSON committed | 51 `.spv`, 21 `reflection/*.json`, ~1.3 MB total; `.gitignore` line 81 notes "Compiled SPIR-V is committed" | Confirmed |
| `ci.yml` comment "`.spv` not committed / gitignored" is outdated | `.github/workflows/ci.yml` `features`, `desktop` comments contradict `.gitignore` | Confirmed |
| 6 jobs each run `Setup slang + compile shaders` | `features`, `lint`, `desktop`, `user-projects`, `dist-size`, `shaders` | Confirmed |
| `dist-size` runs nightly `build-std` full release build on every push to `master`/`dev` | `ci.yml` `on.push.branches` + `if: github.event_name == 'push'` | Confirmed |
| `shaders` job unconditionally recompiles and checks drift | `cache_shaders: "false"`, no path condition | Confirmed |
| `pr-triage` only checks whether body contains `**Result:` line | `pr-triage.yml` `grep -qE '^\*\*Result:'` | Confirmed |
| `pr_ready.py` already outputs `HEAD: \`<short-sha>\``, but no one compares | `scripts/pr_ready.py` self-check block first line | Confirmed |
| `pr_ready` uses `--test-threads=1` for `prism-render` | `crate_test_command()` `#847` handling | Confirmed (not a gap) |
| CI and `pr_ready` crate sets inconsistent | CI uses fixed `-p` list; `pr_ready` per touched crates | Confirmed; impact see §7.3 |
| rust-cache key forks by job | `-lint`, `-desktop`, `-features`, `-projects`, `-shaders` | Confirmed |
| No event-driven agent worker | `.github/workflows/` only has CI, triage, notify, cancel | Confirmed |
| `agent/working` no TTL, no stale cleanup | `tools/ci/ensure_labels.sh` only label definitions | Confirmed |

---

## 3. Design Principles

1. **Inner before outer**: if inner loop output is untrustworthy, outer loop only amplifies garbage. Worker not enabled before P0 done.
2. **Skip must be visible**: `skip` vs `pass` must be distinguishable in output; do not treat "not run" as "passed".
3. **Read-only first**: outer loop first phase only does labels, comments, summaries; no code writing, no PR opening.
4. **Default off**: any automation that writes to repo must have a repo variable switch, and switch takes precedence over trigger conditions.
5. **Single source of truth**: change judgment implemented once (`tools/ci/changed_since.sh`); workflows no longer duplicate diff logic.

---

## 4. Verification Tiers

`pr_ready.py --quick` essence is not "doing less", but trading failure probability for feedback speed. Elevate it to tiered verification across the whole flow:

| Layer | Trigger | Content | Typical duration | Budget position |
|-------|---------|---------|------------------|-----------------|
| **L0 quick** | Agent after each change | `pr_ready.py --quick`: fmt + touched crate clippy | 30–60 s | High-frequency, token-sensitive |
| **L1 PR** | push / PR | quick + touched crate unit tests + independent workspace `--locked` check + shader drift (only when shader changed) | 5–8 min | Required gate |
| **L2 nightly** | **Runs only when new commits** | `features` + `user-projects` + full workspace tests | ~20 min | Low-frequency, anti-drift |
| **L3 release** | tag / version change | `dist-size` + Android packaging + `pr_ready --all` | ~40 min | Pre-release |

Rules:

- L0 is agent's **default first round**, written into `ai-workflow.md`; full run only before opening PR.
- L1 execution set determined by change surface, not job count.
- L2/L3 do not enter PR pipeline; their existence is to prevent "rotting from long non-runs", not every PR's obligation.

---

## 5. P0: Inner Loop Trustworthiness

### 5.1 Self-check block binds HEAD

**Problem**: `pr-triage` only matches `^\*\*Result:`. Expired blocks, handwritten blocks, copied blocks all pass, making "green light" untrustworthy; human review must re-verify.

**Fix**: in `pr-triage.yml` self-check step:

1. Extract `HEAD: \`([0-9a-f]{7,40})\`` from PR body.
2. Compare with `${{ github.event.pull_request.head.sha }}` by **prefix** (`pr_ready` outputs short sha).
3. Missing line or mismatch → add `needs/self-check`, post fixed template:
   > The self-check block is not the result of current HEAD. Please re-run `python scripts/pr_ready.py` on the latest commit and paste the full Markdown block back into the PR body.
4. Match → remove `needs/self-check`.

`pull_request_target` `synchronize` event already covers every push, so each new commit is automatically re-judged; no new trigger condition needed.

**Boundary**: only verify "block belongs to current HEAD", not whether block content is tampered. The latter is handled by CI re-run; this item's goal is to eliminate "expired blocks", not replace CI.

### 5.2 Error code-ification and alternatives

Audit failure messages of `check_deps.py`, `check_features.py`, `check_comment_only.py`, `check_commit_msg.py` item by item, requiring three elements:

1. **Reason**: why prohibited;
2. **Alternative**: what to change to;
3. **Stable error code**: e.g. `DEP001`, `FEAT002`, corresponding to a section in `docs/`.

Not required to complete at once. Criterion: same class of fix appears ≥3 times historically before upgrading to rule or doc; avoid creating new maintenance burden just to "write for agents".

### 5.3 Optional local hook

Add `.githooks/pre-push`, runs `python scripts/pr_ready.py --quick`. Only takes effect after `git config core.hooksPath .githooks`; Termux degrades to `cargo fmt` + `cargo clippy` per `AGENTS.md` authorization.

No `pre-commit`: every commit triggering compilation, interruption cost higher than benefit.

### 5.4 P0 acceptance

- Submit block with old HEAD → rejected;
- Same HEAD repeated paste → passes;
- Weekly `needs/self-check` trigger count trends to 0.

---

## 6. P1: Change Gate and Read-only Patrol

### 6.1 Shared gate script

Add `tools/ci/changed_since.sh`:

```
Usage: changed_since.sh <base> <tip> [commits|version]
Exit code: 0 = need to run; 1 = skip
```

- `commits` (default): `base..tip` has changes outside `docs/`, `*.md`, or `Cargo.toml`, `assets/shaders/**` changed → run.
- `version`: only root `Cargo.toml` `version =` line changed → run.

Existing `ci.yml` doc-only judgment also changed to call this script, avoiding a third diff logic.

### 6.2 Guard's three hard semantics

1. **Baseline from last successful run**: `gh run list -w nightly-automation.yml -s success -L1 --json headSha`, no state file, no extra branch, no `contents: write`.
2. **Failure does not advance baseline**: next run automatically retries, not consumed by marker.
3. **No concurrency**: single `concurrency` group + `cancel-in-progress: false`, cron jitter won't stack two runs.

Additional semantics: `workflow_dispatch` provides `force` input, manual trigger runs unconditionally.

### 6.3 Workflow skeleton

```yaml
name: nightly-automation
on:
  schedule:
    - cron: "0 18 * * *"        # 02:00 CST
  workflow_dispatch:
    inputs:
      force:
        type: boolean
        default: false
      scope:
        type: choice
        options: [commits, version]
        default: commits

concurrency:
  group: nightly-automation
  cancel-in-progress: false

jobs:
  guard:
    runs-on: ubuntu-latest
    outputs:
      run: ${{ steps.gate.outputs.run }}
      base: ${{ steps.gate.outputs.base }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - id: gate
        env:
          GH_TOKEN: ${{ github.token }}
          FORCE: ${{ inputs.force }}
          SCOPE: ${{ inputs.scope || 'commits' }}
        run: |
          set -euo pipefail
          if [ "${FORCE:-false}" = "true" ]; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          tip=$(git rev-parse origin/master)
          last=$(gh run list -w nightly-automation.yml -s success -L1 \
                   --json headSha -q '.[0].headSha')
          if [ -z "$last" ] || [ "$last" = "null" ]; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if [ "$last" = "$tip" ]; then
            echo "run=false" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if bash tools/ci/changed_since.sh "$last" "$tip" "$SCOPE"; then
            echo "run=true" >> "$GITHUB_OUTPUT"
            echo "base=$last" >> "$GITHUB_OUTPUT"
          else
            echo "run=false" >> "$GITHUB_OUTPUT"
          fi
```

All downstream jobs use `needs: guard` and `if: needs.guard.outputs.run == 'true'`.

### 6.4 Read-only patrol tasks

| Task | Judgment | Action | Write permission |
|------|----------|--------|------------------|
| stale claim | `agent/working` and no new comment for 24 h | Comment reminder + switch to `agent/blocked` | `issues: write` |
| ready digest | `agent/ready` and no `agent/working` | Summarize to Discord (reuse existing webhook) | None |
| triage backlog | `needs/triage` over 72 h | Summarize reminder | None |
| Weekly report | Summarize §10 metrics | Write to job summary | None |

This layer deliberately writes no code, opens no PR. Run for two weeks, use data to judge whether worker is worth doing.

### 6.5 Skip semantics

When `guard` judges skip, downstream `skipped`, whole run produces no success record. Intentional: skip does not count as "verified".

Also require writing one line `skipped: no new commits since <sha>` in job summary, to avoid maintainers mistakenly thinking it ran.

---

## 7. CI Budget Governance

### 7.1 Conclusion

**Do not shard repo, do not shard crate, do not increase job count.** What needs granularity is verification tier (§4) and doc granularity (§8). GitHub bills by job·minutes; sharding only copies fixed overheads (checkout, toolchain, apt, cache restore) more times; parallelism buys wall clock, not budget.

### 7.2 Specific list

| Location | Status | Suggestion | Benefit |
|----------|--------|------------|---------|
| `dist-size` | Runs nightly `build-std` full release build on every push to `master`/`dev` | Change to `tags: v*` + `workflow_dispatch`, or connect to §6 `version` gate | Largest single saving |
| `features` / `lint` / `desktop` / `user-projects` / `dist-size` | Each runs `Setup slang + compile shaders` | Delete; `.spv` + reflection already committed (51 + 21 files) | 5 fixed overheads |
| `shaders` | Recompiles 51 shaders on every code change | Run only when `assets/shaders/**` or `crates/xtask/**` changed, or `master` push | Long-tail waste |
| `features` | Runs feature matrix + Windows cross-check on every code change | Run when `Cargo.toml`/feature changed, else L2 | Medium |
| `user-projects` | Builds `projects/*` on every code change | Run when `crates/prism-{app,engine,scene,asset,render}/` or `projects/` changed | Medium |
| rust-cache key | Five jobs five caches | `lint` and `desktop` share one key; `features` keep independent key | Medium |

### 7.3 Relationship with `pr_ready`

`pr_ready.py` already converges per touched crates, direction correct, no shard. But CI `desktop` job uses fixed crate list, two sets inconsistent:

- This itself is acceptable: CI is "baseline", `pr_ready` is "developer loop".
- But must acknowledge in docs that the two are **not equivalent**, avoid treating PR self-check block as CI substitute.
- This plan does not force unification: unification would require CI to call `pr_ready`, losing explicit `--test-threads=1` control and job-level cache isolation; benefit does not outweigh risk.

### 7.4 Risk

After deleting `lint`/`desktop` `setup-slang`, if PR changes `.slang` but does not submit `.spv`, build uses stale bytecode. This situation is covered by `shaders` job drift check (`cache_shaders: "false"` and compares `git status`), will not slip through. Need to fix outdated "`.spv` gitignored" comment in `ci.yml` at the same time.

---

## 8. Documentation and Context Budget

### 8.1 Split `ai-workflow.md`

Current 447 lines, every agent session reads it, largest fixed token overhead.

- `ai-workflow.md`: hard rules, DoD, situations requiring stopping to ask, command quick reference, controlled within 100 lines.
- `ai-workflow-details.md`: examples, templates, anti-patterns item by item, historical cases, read on demand.
- `AGENTS.md` keeps "map + red lines", details external links; no longer grows.

Both files stay at L3, `docs/README.md` topology table registered synchronously.

### 8.2 Minimum must-read set per `area/*`

| area | Must read | Explicitly skip |
|------|-----------|-----------------|
| `area/render` | `GEMINI.md` / `AGENTS.md` coordinate system + Render-graph rules, `slang-bindless-migration.md` | Android, asset pipeline, editor |
| `area/editor` | `editor-egui.md`, `project-module-architecture.md` | Vulkan pass, shader budget |
| `area/android` | `android-apk.md`, `android-arm64.md` | render graph, reflect |
| `area/ci` | `integrations.md`, this plan | All subsystem docs |
| `area/scene` | `REFLECT.md`, `GEMINI.md` / `AGENTS.md` scene/asset layering | Animation, audio |

This table goes into `integrations.md`, driven by `area/*` labels; agent picks issue, looks at label first, then decides what to read.

### 8.3 Feedback discipline

- Test failure reports only failing case name and first assertion difference, not full logs.
- `pr_ready.py`'s `test_summary()` is already correct practice, keep and extend to CI failure summary.
- Agent does not repeat large compile output in PR; artifacts go to artifacts, PR only puts conclusion and paths.

---

## 9. Metrics and Acceptance

| Metric | Collection method | Target |
|--------|-------------------|--------|
| `needs/self-check` weekly trigger count | `gh api` label event stats | Trends to 0 |
| Actual job count executed per common PR | Actions run job list | ≤ 4 |
| Agent single-task round-trip median | Session record sampling | Declines month over month |
| First-round `pr_ready` pass rate | Session record sampling | Rises month over month |
| Nightly skip rate | guard output | Days with nothing to do should be 100% |
| Stale `agent/working` count | Daily patrol | 0 |
| Worker PR first-pass rate (after M4) | PR + CI results | Quality only, not quantity |

---

## 10. Milestones and Landing Order

| Phase | Change | Files | Rollback |
|-------|--------|-------|----------|
| M0 | This plan reviewed | This file | Delete file |
| M1 | Self-check block binds HEAD; first 3 error codes | `pr-triage.yml`, `scripts/check_*.py` | Revert single commit |
| M2 | `changed_since.sh` + `nightly-automation.yml` + 4 read-only tasks | `tools/ci/`, `.github/workflows/` | Turn off workflow |
| M3 | Split `ai-workflow.md`; `area` must-read set | `docs/`, `integrations.md` | Revert |
| M4 | `dist-size` frequency reduction; delete redundant setup-slang; cache key merge | `ci.yml` | Revert |
| M5 | Worker (`AGENT_WORKER_ENABLED=false`) | `.github/workflows/agent-worker.yml` | Variable to false |

M1 and M2 can be parallel; M4 must be after M2, because frequency reduction depends on `changed_since.sh` interface.

---

## 11. Explicitly Not Doing

| Item | Reason |
|------|--------|
| automerge | Conflicts with statement "AI output does not mean maintainer reviewed"; golden frame without GPU shows `skip`, displays green on GitHub; engine regressions are in screen, feel, frame time, need human to see diff |
| fuzzing auto-open issue self-healing loop | Self-amplifying loop, lacks fingerprint dedup and daily cap, would spam; no stable crash fingerprint source currently |
| 7×24 resident agent process | Context pollution, memory corruption, concurrent conflict; event-driven + one-time sandbox is correct form |
| Shard repo / shard crate | Cross-repo sync, dependency release, issue splitting cost exceeds benefit; 24 crates UI-free / Vulkan-free boundary is an asset |
| Shard more CI jobs | Fixed overhead grows linearly with job count, buys only wall clock |
| Worker direct push to `dev` | `dev` direct push is human escape hatch; automated writes must go branch + PR + self-check block |

---

## 12. Risks and Rollback

| Risk | Impact | Mitigation |
|------|--------|------------|
| Guard baseline wrong, long-term skip | Periodic check fails | Job summary outputs `skipped since <sha>`; provide `force` manual entry |
| Delete setup-slang, build with stale `.spv` | Test passes but bytecode outdated | `shaders` job drift check covers; fix outdated comment |
| `dist-size` frequency reduction, volume regression found late | Found over budget only pre-release | Release pipeline forced run; weekly scheduled run |
| Read-only patrol posts misleading comments | Noise | Fixed copy, comment once per category, with `<!-- janitor:... -->` dedup marker |
| Worker and human edit same issue simultaneously | Conflict | Claim label + double confirm + single writer per issue; violation cancels run |
| Label race causes duplicate run | Duplicate PR | `concurrency` + branch name idempotency + open PR check |

All changes are new YAML/scripts or single-point edits; rollback uniformly revert single commit; workflow can be disabled directly on Actions page.

---

## Appendix A: Guard implementation points

1. `fetch-depth: 0` must be enabled, otherwise `changed_since.sh` cannot compare.
2. `gh run list -w` uses workflow filename, avoids failure after rename.
3. Scheduled workflow runs only on default branch; `cron` time in UTC, comment notes local time.
4. `force` input and `schedule` branch both explicitly handled, cannot rely on `inputs` empty default.
5. All downstream jobs must carry `needs: guard` + `if:`; missing one bypasses the gate.

## Appendix B: Labels and permissions

New labels:

| Label | Meaning | Created at |
|-------|---------|------------|
| `agent/fix` | Issues worker may take | `tools/ci/ensure_labels.sh` |
| `automation` | Items produced or handled by periodic task | Same |

Permission minimization:

| Workflow | Permissions |
|----------|-------------|
| `pr-triage` | `contents: read`, `pull-requests: write`, `issues: write` |
| `nightly-automation` | `contents: read`, `issues: write`; read-only tasks don't need `contents: write` |
| `agent-worker` | `contents: write`, `pull-requests: write`, `issues: write`, subject to repo variable switch |
