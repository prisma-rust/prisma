# Agent contract audit: external template vs repository status

This file is a **snapshot-style health check report**: comparing an external "production-grade `AGENTS.md` template" (zero comments, verification-driven, Git automation circuit breaker) line by line against
the rules already established in this repository, giving three types of conclusions——surpassed the template, conflicts with current status, still empty——and listing pending decisions.

- Audit baseline: `dev` @ `76b1068`, working tree clean, `core.hooksPath=.githooks` installed
- Audit script basis: [../AGENTS.md](../AGENTS.md), [ai-workflow.md](ai-workflow.md), [code-style.md](code-style.md),
  [../.github/pull_request_template.md](../.github/pull_request_template.md), `scripts/` measured output
- Data are all reproducible with commands, statistics scope written at the beginning of each section; this file **does not describe** unverified speculation

---

## 1. Conclusion summary

| Template section | Repository status | Disposition |
|---|---|---|
| §1 Zero comments / cleanup of existing comments | **Opposite direction**: specification explicitly requires retaining invariant comments | Reject copying, take the "comment budget" route |
| §2 Architecture and coding iron laws | Layering/footgun locked by tools; "zero masking" not achieved | Partially adopted |
| §3 Verification first | Stronger than template (single-command gate + hook + CI), but missing "user flow" and PR-level property tests | Patch gaps, no structural change |
| §4 PR delivery contract | **Structural conflict**: repository template bound to pr-triage mechanical verification | Add only, no replacement |
| §5 Git hygiene and circuit breaker | Already has harder rules; "prohibit automerge" has no object | Rewrite as review circuit breaker |

In one sentence: most of this template's "soft constraint" part has already been replaced by tools in this repository; directly landing it would break the existing gates.

---

## 2. §1 Zero comments: not met, and direction opposite

| Template requirement | Repository rule | Location |
|---|---|---|
| Prohibit all non-doc comments | Require retaining `// SAFETY:`, VUID, coordinate conventions, binary layout, lock order | [code-style.md](code-style.md) §2 |
| Touch file, clean surrounding comments | Comment cleanup must be **separated** from logic changes, otherwise `check_comment_only.py` fails | [code-style.md](code-style.md) §7 |
| Prohibit TODO/FIXME | Met | See below |

**Existing measurement** (`python scripts/check_comments.py --all`, exit code 1):

```text
comments: 12541 finding(s) in 401 file(s) (cjk=11626, separator=818, todo=0, allow=97)
```

- `cjk=11626`: Code comments are still Chinese, conflicting with the "comments must be English" specification, concentrated in early crates such as `prism-app`, `prism-cooker`, `prism-anim`
- `separator=818`: Decorative separator lines still present (`// ----------`, `// Entity`)
- `todo=0`: `TODO/FIXME` cleared; the 9 `XXXX` hits in `crates/**/*.rs` across the repository are all garbage byte literals in tests (`from_bytes(b"XXXX")`)
- `allow=97`: Bare `#[allow(...)]` without `reason = "..."`

**Gate status**: [../.github/workflows/ci.yml](../.github/workflows/ci.yml) and [pr_ready.py](../scripts/pr_ready.py) only scan **new/modified lines** (`--added-lines`), so existing items do not block, but also means **existing items will never be cleaned**—unless a budget mechanism is introduced.

**Rejected suggestion**: The template companion suggests adding a regex block on `//` in CI. In this repository it would cause catastrophic false positives: `//` is the carrier of `//!` module docs and `// SAFETY:`; `#` is a valid leading token in `scripts/*.py` (paths referenced in sections 2 and 3 of this file all contain `#`).

---

## 3. §2 Architecture and coding iron laws: half solidified, half slogans

### 3.1 Already stronger than the template (tools locked in)

| Focus | Tool | Entry |
|---|---|---|
| Layering and dependency direction | `check_deps.py`, `check_arch_guards.py`, `cargo tree | grep -i egui` | [pr_ready.py](../scripts/pr_ready.py) `check_arch_guards` |
| High-risk low-level API | `.clippy.toml` `disallowed-types` / `disallowed-methods` | [../.clippy.toml](../.clippy.toml) |
| Feature matrix and dependency budget | `check_features.py`, `validate_feature_map.py` | CI `features` job |
| Artifact size | `dist-size` (nightly build-std) | CI |

`.clippy.toml` has banned `std::any::TypeId`, `std::env::current_dir`, `std::env::set_current_dir`, and provides
the **controlled escape hatch** `#[expect(clippy::disallowed_methods, reason = "...")]`. This is more enforceable than the template's "prohibit footgun".

### 3.2 Not met

**(a) "Zero masking" not achieved.** Scan scope `#!?\[allow\(`:

- `#[allow(...)]` / `#![allow(...)]`: **112 occurrences / 80 files**
- `#[expect(...)]`: **9 occurrences / 9 files**
- Of the 71 categorized occurrences: `dead_code=33` (22 files), `clippy::too_many_arguments=24`,
  `clippy::macro_metavars_in_unsafe=3`, others scattered

`#[allow(dead_code)]` is the category most worth tightening: it covers up "dead code left by temporary hacks", directly conflicting with "zero compatibility burden before 1.0" ([versioning.md](versioning.md)).

**(b) "Warnings are hard errors" only applies to changed crates.**

- [pr_ready.py](../scripts/pr_ready.py) runs `clippy --all-targets -- -D warnings` only on touched crates
- CI `lint` job covers only five desktop crates `prism-ecs prism-gfx prism-render prism-engine prism-platform` + `xtask`
- `pr_ready.py`'s `SKIP_BY_DEFAULT = {"prism-app"}`: covered by `projects/template` check
- `#![deny(warnings)]` exists in only 5 crates: `prism-ecs`, `prism-engine`, `prism-physics2d`, `prism-platform`, `prism-render`

**(c) "Treat root cause, ban timers" has ready counterexamples (product code, not tests):**

| Location | Status | Nature |
|---|---|---|
| [crates/prism-editor-mcp/src/game.rs](../crates/prism-editor-mcp/src/game.rs) | 200ms sleep + two `metadata().len()` checks to determine screenshot write completion | Using time to mask file lifecycle boundary |
| [crates/prism-editor-mcp/src/editor_egui.rs](../crates/prism-editor-mcp/src/editor_egui.rs) | 200ms polling for control channel readiness | Race masked by polling |
| [crates/prism-app/src/render_runner.rs](../crates/prism-app/src/render_runner.rs) | 50ms sleep when not present | Reasonable backpressure |
| [crates/prism-app/src/app/mod.rs](../crates/prism-app/src/app/mod.rs) | headless frame wait 10ms×100 | Reasonable bounded wait |
| [crates/prism-asset/src/runtime/hot_reload.rs](../crates/prism-asset/src/runtime/hot_reload.rs) | Polling thread `thread::sleep(interval)` | Polling is this module's design contract |

`sleep`-related hits in `crates/**/*.rs`: 53 occurrences / 18 files, mostly concentrated in `#[cfg(test)]`.

**(d) "Determinism / hot-path unbounded allocation" has no gate at all**, currently just prose clauses.

---

## 4. §3 Verification first: strongest item, but "runtime evidence" has no teeth

**Solidified**: `python scripts/pr_ready.py` one command chains fmt, comments, version sync, clippy, test, shader-bindgen drift,
editor check, projects check, docs naming, architecture guard, feature map, dependency budget, outputting a Markdown block
ready to paste into PRs (`HEAD: <sha>` + `**Result:**`). `.githooks/pre-push` enforces the same gate locally; failure exits non-zero.

**Not met:**

1. **Gate does not run "user flows"**: `pr_ready.py` only runs unit tests, not headless driving, screenshots, or benchmarks.
   `PRISMA_BENCH_REPORT` / `PRISMA_SCREENSHOT` / `prism-app::run_headless` are only written as text requirements in
   [../AGENTS.md](../AGENTS.md) §1, **with no script verifying that PRs actually attach these artifacts**.
2. **Property tests only on nightly**: [nightly-verify.yml](../.github/workflows/nightly-verify.yml)'s
   `proptest_invariants` (`prism-ini`/`prism-physics2d`/`prism-reflect`/`prism-scene`) is not in the PR loop.
3. Self-healing loop achieved: gate failure exits 1, and AGENTS §1 explicitly orders "never edit the check".

---

## 5. §4 PR delivery contract: structural conflict

| Template requirement | Repository reality |
|---|---|
| `## Intent & State Change` / `## Verification Artifacts` / `## Blast Radius & Risk` | `What / Why`, `Change Type`, `Verification Evidence`, `Self-check`, `AI Attribution`, `Risks & Rollback` |
| No AI declaration requirement | [ai-workflow.md](ai-workflow.md) §4 mandates first-line AI attribution + `🤖 Agent:` prefix |
| No self-check block mentioned | [pr-triage.yml](../.github/workflows/pr-triage.yml) mechanical verification requires body to contain `**Result:` and `HEAD: \`<sha>\`` matching current head, otherwise tags `needs/self-check` |

If replacing the PR template with the template, **pr-triage would immediately lose guard**, and lose AI attribution compliance.

What's truly worth adding in the template, and the repository currently has no machine verification for, is **Blast Radius (changed module list)** and checklist checking:
`Verification Evidence` left empty still passes triage.

---

## 6. §5 Git hygiene and circuit breaker: two not applicable + one reverse gap

- **Solidified**: Conventional Commits and branch naming (`check_commit_msg.py` mounted on both `commit-msg` hook and pr-triage),
  atomic commits, `fetch + rebase + --force-with-lease`.
- **"Prevent automerge" not applicable**: Full repository search `automerge|auto-merge|gh pr merge --auto` **0 hits**, repository has no automerge.
  Circuit breaker semantics need to be rewritten as "cannot merge without required approval".
- **"Every change must go through PR" conflicts with reality**: [../AGENTS.md](../AGENTS.md) §7 allows `dev` direct atomic commit pushes, only
  `dev → master` goes through PR.
- **Upgrade trigger condition not covered**: [../.github/CODEOWNERS](../.github/CODEOWNERS) only covers `/.github/`, `/tools/ci/`,
  `scripts/pr_ready.py`, `scripts/check_commit_msg.py`, and notes "Enforced once branch protection enables CODEOWNERS review". Changing RSCN / RTTIBin / public API **will not** automatically pull people.
- **Reverse gap**: The template does not have one harder rule already in this repository—[../AGENTS.md](../AGENTS.md) §7 "destructive commands individually manually approved + uncommitted work is unrecoverable".

---

## 7. Decided

1. **Branch and gate granularity**: Keep `dev` direct pushes. Each `dev` commit only runs the inner loop `--quick` (fmt + clippy);
   full gate runs only before `master` commit or opening `dev → master` PR. Landing points: [../AGENTS.md](../AGENTS.md) §1,
   [ai-workflow.md](ai-workflow.md) §1/§2, [../.githooks/pre-push](../.githooks/pre-push).
2. **Comment strategy**: Changed to "touch and clean"—when modifying a file, clean up comment garbage in and around the modified area (outdated comments,
   code repetition, commented-out old code, decorative separators, Chinese comments translated to English). **Repository-wide** comment cleanup remains a separate comment PR,
   proven by `check_comment_only.py` to have zero code changes. Landing points: [../AGENTS.md](../AGENTS.md) §8,
   [code-style.md](code-style.md) §7.

Still pending decisions:

1. Whether to introduce a stock budget for `check_comments.py --all` (current stock 12541 items; only gating new lines means never converging)?
2. Whether to convert the 33 `#[allow(dead_code)]` to `#[expect(..., reason)]` or delete the code directly?

Remaining actionable items (sorted by cost-effectiveness):

1. Build a baseline file for `check_comments.py --all`, `cjk` / `separator` only decrease, never increase
2. Convert 33 `#[allow(dead_code)]` to `#[expect(..., reason)]` or delete code directly; gate adds "new `#[allow(` must bring reason"
3. pr-triage verifies `## Verification Evidence` non-empty and contains command or log path
4. CODEOWNERS supplements save format / public API / `Cargo.toml` paths, using review instead of non-existent automerge
5. `game.rs` screenshot wait changed to process exit + file stability event; `editor_egui.rs` changed to control port readiness signal
6. PR-level `proptest_invariants` (target crates already verified runnable on nightly)
