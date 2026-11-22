# Code comment and doc comment specification

Applies to **code comments** in `prisma` repository: `.rs` `//` / `///` / `//!` / `/* */`, and comments in `Cargo.toml`, CI YAML, shell / Python / PowerShell, `.slang`.

**Two goals only: comments in English; delete comments that provide no information.**
(This document itself, `docs/**/*.md`, `README.md`, `AGENTS.md` continue using Chinese, not within this specification.)

## 1. Language boundary

| Content | Language |
|-------|----------|
| Rust code comments, rustdoc | English |
| `Cargo.toml` / CI / build scripts / shader comments | English |
| `docs/**/*.md`, `README.md`, `AGENTS.md`, `CONTRIBUTING.md` | Chinese |
| UI copy, logs, error messages, user-visible text in test assertions | Keep original (usually Chinese) |
| Identifiers, type names, commands, paths, Vulkan enums, format names | Do not translate |

Technical terms follow existing repository writing: `swapchain`, `bindless`, `render graph`, `bind group`, `descriptor set`, `push constant`, `RSCN`, `RTTIBin`, etc. Do not invent Chinese translations.

### Why emphasize this

Historically one commit (`803c0068`) used a script to **word-by-word replace** English comments into Chinese, leaving many broken sentences:

```rust
/// Raw index into the entity allocator. Stable for the entity's lifetime.
/// Monotonically increasing version; bumped each time the slot is recycled.
```

This text is harder to read than the English original, and harder to maintain. When restoring, prefer querying git history before that commit, then rewrite per current code; **do not use replacement scripts to generate final comments**.

## 2. What to keep

Only keep information "not obvious from reading code":

- Module-level `//!`: what this module is responsible for, where boundaries are, shared conventions with whom.
- Public item `///`: semantics, units, invariants, when panic / error occurs, conditions callers must satisfy.
- Non-obvious algorithms, math, coordinate conventions (especially matrix and coordinate system conventions in `AGENTS.md`).
- Vulkan spec constraints, VUID, driver workarounds, layout transitions, sync and lifecycle invariants.
- Binary format layout (magic / version / field order); note 1.0 does not maintain old binary compatibility, directly upgrade format and remove old logic.
- Thread model, ownership, lock order, channel semantics.
- Performance tradeoffs, feature tradeoff reasons (e.g. "why this dependency must be optional").
- Truly non-obvious `unsafe` preconditions, explained with `// SAFETY:`.

## 3. What to delete

- Comments restating code (`// increment i`, `// return the value`).
- Decorative separators (`// ----------`, `// ==========`, `// Entity` pure titles).
- Commented-out old code.
- Before 1.0 release (current 0.1), dead code, compatibility facades, old branch fallbacks, and shims retained for old version compatibility (old version compatibility explicitly regarded as redundant garbage code).
- Expired status / milestone / "currently" / "temporarily" / "later" explanations without issue links.
- Content repeated multiple times in module doc, type doc, field doc for the same fact.
- `TODO` / `FIXME` / `XXX` / `HACK` without issue or clear reason.
- Abbreviations, code names, internal jokes only meaningful to the author.

When long files need "section sense", prefer splitting modules or functions, not adding separators.

## 4. rustdoc writing

Public items use `///`, first line is summary, blank line then details:

```rust
/// Returns the entity's generation.
///
/// The generation distinguishes a live handle from a stale one after the
/// slot is recycled. It is not a global version counter.
pub fn generation(self) -> u32 {
```

Use standard paragraphs as needed:

```rust
/// Decodes an RSCN scene from `bytes`.
///
/// # Errors
///
/// Returns [`SceneError::UnsupportedVersion`] when the header version is newer
/// than this build supports.
///
/// # Panics
///
/// Panics when `bytes` is shorter than the fixed header.
```

- First line uses complete sentence, ends with period.
- Identifiers, paths, commands in backticks; Rust items use rustdoc links (`` [`World`] ``).
- Do not write dates, authors, change history; those belong to git / CHANGELOG.
- Single line not over 100 characters, consistent with rustfmt line width.
- Code blocks in docs must be compilable (doctest) or explicitly marked `text`.

## 5. `SAFETY`, `TODO` and formatting

### `SAFETY`

`unsafe` blocks only write `SAFETY` when preconditions are not obvious, close to code:

```rust
// SAFETY: `image` was created from `device` and is still alive; the caller
// holds `&mut self`, so no other submission can destroy it concurrently.
unsafe { device.destroy_image(image, None) };
```

Do not mechanically add "caller guarantees safety" for every `unsafe`; write clearly the **specific** invariant.

### `TODO` / `FIXME`

Must carry issue or link:

```rust
// TODO(#123): replace the linear scan once the asset index lands.
```

Without issue, delete it; do not leave to-dos in comments as hidden requirements.

### `#[allow(...)]` and `#[expect(...)]`

`#[allow(...)]` / `#![allow(...)]` are **banned in this repository, no exceptions**: `dead_code` is not a reason, `clippy::too_many_arguments` is not a reason, generated code is not a reason. When a lint fires, delete the code it points at, or change the design until the lint stops firing — needing long-term suppression itself proves the design is wrong.

The only escape hatch is `#[expect(<lint>, reason = "<reason>")]`: it documents "here deliberately so", and **fails the build once the lint stops firing**, so it cannot rot into dead weight. Only use where the lint is wrong about correct code (FFI signatures, Vulkan handles); never to silence `dead_code`.

```rust
// Allowed: long parameter list comes from fixed C ABI shape.
#[expect(clippy::too_many_arguments, reason = "mirrors the C ABI signature")]
pub unsafe extern "C" fn create(...) {}

// Prohibited: lint says this code is unused, delete it.
#[allow(dead_code)]
fn legacy_path() {}
```

### Warnings are errors

The entire workspace must pass `cargo clippy --all-targets -- -D warnings`, **not** only crates touched by this commit. A warning in an untouched crate is still a defect: fix it or delete the code.

### Separators

Do not use:

```rust
// ---------------------------------------------------------------------------
// Entity
// ---------------------------------------------------------------------------
```

Organize code directly with modules / functions / types; if navigation needed, use `//!` module doc lists.

## 6. Examples

Keep (explains why):

```rust
// The framebuffers must be destroyed before the swapchain is recreated,
// otherwise validation reports VUID-vkDestroyFramebuffer-... and the device
// can be lost on some drivers.
```

Delete (just restates code):

```rust
// Set the width.
self.width = width;
```

Delete (expired milestone):

```rust
// This is a skeleton for milestone 1; later milestones can slot in RenderSystem.
```

When keeping, rewrite into English:

```rust
// Before
/// Raw index into the entity allocator. Stable for the entity's lifetime.

// After
/// Raw index into the entity allocator. Stable for the entity's lifetime.
```

## 7. Comment-only PR

Daily changes apply "touch it, clean it" (see [`AGENTS.md`](../AGENTS.md) §8): when modifying a file, delete outdated comments, restating-code comments, commented-out old code, decorative separators in **the modified area and surroundings**, rewrite Chinese comments into English. This is part of the same commit, do not leave for a later sweep.

**Repository-wide** comment sweep is still a separate comment PR, separated from logic changes. Each comment PR runs:

```bash
python scripts/check_comment_only.py --base origin/master
```

It uses the same Rust lexical scan to strip comments and blanks, compares remaining code tokens. Any code change fails; pure comment modification (including deleting comments) passes. During full repository scan, do not incidentally change code, reorder imports, or run full rustfmt; "touch it, clean it" is not subject to this because it is itself part of that logic change.

If comment fix must accompany code change (e.g. bad link points to item that no longer exists), split code change into another PR, or clearly explain in PR and ask reviewer to focus there.

## 8. Tools

```bash
# Full repository check (migration period outputs existing, for statistics)
python scripts/check_comments.py --all

# Only check changes relative to base
python scripts/check_comments.py --changed --base origin/master

# Only check staged area; output Markdown rows ready to paste into PR
python scripts/check_comments.py --staged --markdown

# A specific file / directory
python scripts/check_comments.py crates/prism-ecs/src/lib.rs
```

`check_comments.py` flags:

- CJK characters in comments;
- Decorative separators;
- `TODO` / `FIXME` / `XXX` / `HACK` without issue;
- Any `#[allow(...)]` / `#![allow(...)]` (all prohibited, change to `#[expect(..., reason = "...")]`);
- `#[expect(...)]` without `reason = "..."`.

Chinese in strings (UI copy, error messages) is not flagged; generated files (`crates/prism-render/src/shader_bindings/`, `launcher/src-tauri/gen/`, `.local/tools/slang/`) are skipped.

## 9. Migration and gate

- Existing Chinese comments and redundant explanatory comments cleaned up batch by batch by crate / module, or cleaned incidentally when touching files.
- `pr_ready.py` passes `check_comments.py --changed --added-lines` to only check **newly added or modified lines**, blocking merge on new violations.
- Each comment PR additionally runs `RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps` to ensure rustdoc links and code blocks are not broken.
- Generated doc comments should not be hand-edited: `shader_bindings/`, Tauri `gen/`, etc. follow the generator.

## 10. Review checklist

- [ ] Comments are English, identifiers / paths unchanged.
- [ ] No restating code, decorative lines, commented-out old code, expired milestones.
- [ ] Non-obvious invariants, units, panic / error / safety conditions retained.
- [ ] `TODO` carries issue; without issue deleted; `#[allow(...)]` carries `reason`.
- [ ] Comment-only PR passes `check_comment_only.py`.
- [ ] `cargo doc -D warnings` passes for modified crates.
