# Comment, API doc, and refactoring boundary specification

Applies to **all source and build files** in the repository: Rust (`.rs`, including `//`, `///`, `//!`, `/* */`),
`Cargo.toml`, CI YAML, shell / Python / PowerShell scripts, and `.slang`. This document is the **single
authority** for comment rules; `AGENTS.md` §8 only points here.

## 1. Policy: zero inline comments

- **Inline comments are banned.** Do not write `//`, `/* */`, or commented-out code in any source or
  build file. Do not write `TODO` / `FIXME` / `XXX` / `HACK` markers anywhere in code; track work in
  GitHub issues instead of leaving hidden requirements in the tree.
- **Rust has exactly one comment form: rustdoc** (`///` outer, `//!` inner). Public API semantics,
  invariants and preconditions go there; a fact that cannot be expressed in rustdoc does not belong
  in the code at all.
- **Non-Rust files carry no explanatory comments.** Configuration and shader intent belongs in the
  surrounding type/field names, or in a document under `docs/`. Do not translate an inline comment
  into another inline comment — delete it and move the information or refactor.
- **Everything is English.** Code comments, rustdoc, docs, `README.md`, `AGENTS.md`, commit bodies and
  PR bodies are English. UI copy, logs, error messages and user-visible test strings keep their
  original language (usually Chinese). Identifiers, paths, Vulkan enums and format names are never
  translated.
- **If a fact needs a comment to be understood, refactor the code** (strong static types, explicit
  state enums, precise names, pure functions) or record it in rustdoc / `docs/`. Do not replace that
  with a shorter inline comment.

## 2. What belongs in rustdoc

Only information that is not obvious from reading the code:

- Module-level `//!`: what the module owns, where its boundaries are, shared conventions with other
  modules.
- Public item `///`: semantics, units, invariants, panic / error conditions, and conditions callers
  must satisfy. For `unsafe` functions use a `# Safety` section describing the **specific**
  preconditions; do not write generic "caller guarantees safety" prose.
- Vulkan spec constraints, VUIDs, driver workarounds, layout transitions, synchronization and
  resource-lifecycle invariants.
- Binary format layout (magic / version / field order). Before 1.0 (currently 0.1) there is no old
  format compatibility: upgrade the format directly and remove old logic.
- Thread model, ownership, lock order, channel semantics.
- Performance tradeoffs and why a dependency or capability is optional.
- Truly non-obvious algorithm / math steps and coordinate conventions (the rendering conventions
  in `AGENTS.md` §4 remain authoritative).

Generated doc comments (`crates/prism-render/src/shader_bindings/`, Tauri `gen/`, etc.) are never
hand-edited; their generators own them.

## 3. What is banned

- Every inline / block comment in Rust and in non-Rust source files.
- `TODO` / `FIXME` / `XXX` / `HACK` markers, with or without an issue number. Use the issue tracker.
- Comments that restate code (`Set the width.`), narrate a diff, or justify a bad implementation.
- Decorative separators (`// ----------`, `// ==========`, bare `// Entity` titles). Use modules and
  types for navigation; module docs may list structure.
- Commented-out old code, dead code, compatibility facades, old-branch fallbacks and shims kept for
  old-version compatibility.
- Expired status / milestone / "currently" / "temporarily" notes without a living spec.
- The same fact repeated in module docs, type docs and field docs.
- Abbreviations, code names and internal jokes meaningful only to the author.

## 4. Rustdoc writing rules

- Public items use `///`; the first line is a complete sentence ending in a period, then a blank line
  with details.
- Use `# Errors` and `# Panics` sections when applicable, and `# Safety` for unsafe preconditions.
- Wrap identifiers, paths and commands in backticks; link Rust items with `` [`World`] ``.
- Do not write dates, authors or change history; those belong to git / CHANGELOG.
- Keep lines under 100 characters, consistent with rustfmt.
- Code blocks must compile (doctest) or be explicitly marked `text`.

## 5. Refactoring boundary (aligned with the zero-comment rule)

The default stays **touch only the code path that produces the bug**. The zero-comment policy adds
one bounded exception:

- The touchable region is the failing path **plus its immediate call site / callee**. Inside it you
  may do the minimum refactor needed to remove an inline comment, express the invariant in types or
  rustdoc, or comply with an architectural invariant (for example extracting a small helper instead
  of leaving a workaround).
- **No unrelated files, modules, imports, formatting-only churn or full-file sweeps.** If removing a
  comment or fixing a violation genuinely requires a wider refactor, stop and split it into a
  separate PR / ask first.
- Never replicate a legacy anti-pattern because it is nearby. If the surrounding pattern violates
  current rules and you must call it, wrap it in a compliant helper within the touchable region or
  report the blocker; do not silently expand the diff.
- This exception does not license opportunistic refactors. "It would be nicer" is not a reason;
  "the change cannot satisfy the zero-comment / invariant rule without it" is.

## 6. Touch it, clean it

A file you modify is yours to garden. Inside the touchable region delete stale comments,
restating comments, commented-out code and decorative separators, and move any still-useful fact
into rustdoc or `docs/`. Do it in the same edit as the code change; never leave it for a later sweep.

A **repository-wide** comment cleanup stays a separate comment-only PR and must pass:

```bash
python scripts/check_comment_only.py --base origin/master
```

The lexical scan drops comments and whitespace and compares the remaining code tokens, so any code
change fails the check. Do not incidentally change code, reorder imports or run full rustfmt in a
comment-only PR. Each comment PR also runs
`RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps` for the affected crates.

If a comment fix must accompany a code change (for example a rustdoc link points to an item that no
longer exists), split the code change into another PR, or call it out explicitly in the PR body.

## 7. Warning suppression (unchanged)

`#[allow(...)]` / `#![allow(...)]` are **banned without exception**: `dead_code` is not a reason,
`clippy::too_many_arguments` is not a reason, generated code is not a reason. Delete the code or
change the design. The only escape hatch is `#[expect(<lint>, reason = "...")]`, which fails the
build once the lint stops firing; use it only where the lint is wrong about correct code (FFI
signatures, Vulkan handles). The single allowed exception is a shared test harness under `tests/`
compiled into several test binaries, which may carry `#![allow(dead_code)]` because `dead_code` is
reported per binary.

The whole workspace must pass `cargo clippy --all-targets -- -D warnings`, not just touched crates.

## 8. Tools

```bash
# Full repository check (legacy inline comments are reported during migration)
python scripts/check_comments.py --all

# Only changes relative to a base
python scripts/check_comments.py --changed --base origin/master

# Only staged area, with a Markdown row for the PR
python scripts/check_comments.py --staged --markdown

# A specific file / directory
python scripts/check_comments.py crates/prism-ecs/src/lib.rs
```

`check_comments.py` flags:

- any inline / block comment (rustdoc is exempt);
- CJK characters in rustdoc (documentation is English);
- decorative separators;
- any `TODO` / `FIXME` / `XXX` / `HACK` marker in code comments;
- any `#[allow(...)]` / `#![allow(...)]` and any `#[expect(...)]` without `reason = "..."`.

Strings and user-facing text are not comments, so Chinese UI strings and error messages are not
flagged. Generated files are skipped.

## 9. Migration and gate

- Existing inline comments are removed gradually: by crate / module in comment-only PRs, or
  incidentally when touching a file. This round changes the policy and the gate only; no bulk source
  cleanup is done.
- `pr_ready.py` runs `check_comments.py --changed --added-lines`, so **new** violations on
  added/modified Rust lines block the merge while legacy comments in untouched lines do not.
  The script gate covers Rust; the same zero-inline-comment rule for non-Rust files is enforced by
  review until equivalent tooling exists.
- Comment PRs additionally run rustdoc with `-D warnings` to catch broken links and code blocks.
- The `dev` inner loop uses `--base origin/dev`, so legacy comments are not re-litigated on every
  commit. The full gate for a `dev → master` PR uses `--base origin/master`; until the migration is
  done it will report inline comments introduced by older commits in that diff. Those must be
  cleaned in a comment-only PR (or converted to rustdoc) before the PR can pass.

## 10. Review checklist

- [ ] No inline / block comments; only rustdoc (`///` / `//!`) remains in changed Rust code.
- [ ] Non-Rust files contain no explanatory comments.
- [ ] Rustdoc is English, carries invariants, units, panic / error / `# Safety` conditions.
- [ ] No `TODO` markers; work is tracked in issues.
- [ ] No restating comments, decorative lines, commented-out old code or expired milestones.
- [ ] `#[allow(...)]` absent; `#[expect(...)]` has `reason`; warnings are errors.
- [ ] Refactors stay inside the touchable region (§5); wider changes are a separate PR.
- [ ] Comment-only PR passes `check_comment_only.py` and rustdoc `-D warnings`.
