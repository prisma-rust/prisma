#!/usr/bin/env python3
"""Check code comments against `docs/code-style.md`.

    python scripts/check_comments.py --all
    python scripts/check_comments.py --changed --base origin/master
    python scripts/check_comments.py --staged --markdown
    python scripts/check_comments.py crates/prism-ecs/src/lib.rs

What it flags:
  * any inline / block comment in Rust (only rustdoc `///` / `//!` is allowed);
  * CJK characters in rustdoc (documentation must be English);
  * decorative separators such as `// ----------`;
  * any `TODO` / `FIXME` / `XXX` / `HACK` marker in code comments;
  * any `#[allow(...)]` / `#![allow(...)]` (banned outright, see `docs/code-style.md`);
  * `#[expect(...)]` / `#![expect(...)]` without `reason = "..."`.

Strings and user-facing text are not comments, so Chinese UI strings and
error messages are not flagged. Generated files such as shader bindings and
Tauri codegen output are skipped.

Use `--added-lines` with `--changed` or `--staged` to check only lines added
or modified in the current diff, turning the check into a hard gate without
failing on legacy comments in untouched lines.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib.console import force_utf8_streams  # noqa: E402
from lib.rust_comments import comment_body, lex  # noqa: E402

force_utf8_streams()

# CJK ideographs, CJK punctuation and fullwidth forms. Typographic symbols
# such as arrows or em dashes are allowed in English prose.
CJK_RE = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\u3000-\u303f\uff01-\uff60]")
SEPARATOR_RE = re.compile(r"^[\s\-=_*#~`^+─━┄┅┈┉…·.]+$")
TODO_RE = re.compile(r"\b(TODO|FIXME|XXX|HACK)\b")
ATTR_RE = re.compile(r"#!?\[\s*(allow|expect)\s*\(")
REASON_RE = re.compile(r"\breason\s*=")
HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")

EXCLUDED_PREFIXES = (
    "crates/prism-render/src/shader_bindings/",
    "launcher/src-tauri/gen/",
    ".local/tools/slang/",
)

# A shared test harness is compiled into many test binaries and every binary
# exercises a different subset of it. `dead_code` is reported per binary, so no
# `#[expect(...)]` can be honest about it: the item is live in one binary and dead
# in another. `#![allow(dead_code)]` is the only attribute that expresses this,
# so it is permitted in `tests/**` modules and nowhere else.
SHARED_HARNESS_ALLOW = re.compile(r"^#!\[allow\(dead_code\)\]$")


def git(*args: str) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=str(ROOT),
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        return 127, ""
    return proc.returncode, proc.stdout or ""


def normalize(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def is_excluded(path: str) -> bool:
    normalized = normalize(path)
    if normalized.startswith(EXCLUDED_PREFIXES):
        return True
    if normalized.startswith("target/") or "/target/" in normalized:
        return True
    if normalized.startswith("node_modules/") or "/node_modules/" in normalized:
        return True
    if normalized.startswith(".zcode/"):
        return True
    return False


def display_path(path: str) -> str:
    resolved = Path(path)
    if not resolved.is_absolute():
        resolved = ROOT / resolved
    try:
        return resolved.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return resolved.as_posix()


def resolve_base(explicit: str | None) -> str:
    if explicit:
        return explicit
    for ref in ("origin/master", "origin/main", "master", "main"):
        if git("rev-parse", "--verify", "--quiet", ref)[0] == 0:
            return ref
    return "HEAD"


def changed_files(base: str) -> list[str]:
    files: set[str] = set()
    probes = (
        ("diff", "--name-only", "--diff-filter=ACMRTUXB", f"{base}...HEAD"),
        ("diff", "--name-only", "--diff-filter=ACMRTUXB", "--cached"),
        ("diff", "--name-only", "--diff-filter=ACMRTUXB"),
        ("ls-files", "--others", "--exclude-standard"),
    )
    for probe in probes:
        code, output = git(*probe)
        if code != 0:
            continue
        files.update(line.strip() for line in output.splitlines() if line.strip())
    return sorted(path for path in files if path.endswith(".rs") and not is_excluded(path))


def staged_files() -> list[str]:
    code, output = git("diff", "--name-only", "--diff-filter=ACMRTUXB", "--cached")
    if code != 0:
        return []
    return sorted(
        line.strip()
        for line in output.splitlines()
        if line.strip().endswith(".rs") and not is_excluded(line.strip())
    )


def tracked_files() -> list[str]:
    code, output = git("ls-files", "-z", "--", "*.rs")
    if code == 0:
        return sorted(path for path in output.split("\0") if path and not is_excluded(path))

    # Not a git checkout (for example a source tarball): walk the tree.
    return sorted(
        path.relative_to(ROOT).as_posix()
        for path in ROOT.rglob("*.rs")
        if not is_excluded(path.relative_to(ROOT).as_posix())
    )


def explicit_files(paths: list[str]) -> list[str]:
    result: set[str] = set()
    for raw in paths:
        path = Path(raw)
        if not path.is_absolute():
            path = ROOT / path
        if path.is_dir():
            for child in path.rglob("*.rs"):
                rel = child.relative_to(ROOT).as_posix() if child.is_relative_to(ROOT) else child.as_posix()
                if not is_excluded(rel):
                    result.add(rel)
        elif path.suffix == ".rs" and not is_excluded(display_path(raw)):
            result.add(display_path(raw))
    return sorted(result)


def is_separator(body: str) -> bool:
    compact = body.strip()
    return len(compact) >= 4 and bool(SEPARATOR_RE.fullmatch(compact))


def first_line(body: str, limit: int = 100) -> str:
    line = body.strip().splitlines()[0] if body.strip() else ""
    return line if len(line) <= limit else line[: limit - 1] + "…"


def diff_added_lines(probe: tuple[str, ...]) -> set[int]:
    code, output = git(*probe)
    if code != 0:
        return set()
    lines: set[int] = set()
    for raw in output.splitlines():
        match = HUNK_RE.match(raw)
        if not match:
            continue
        start = int(match.group(1))
        count = int(match.group(2)) if match.group(2) is not None else 1
        if count > 0:
            lines.update(range(start, start + count))
    return lines


def added_line_numbers(rel_path: str, base: str | None, staged_only: bool) -> set[int] | None:
    """Return 1-based line numbers added/modified in git, or None for untracked files."""
    code, untracked = git("ls-files", "--others", "--exclude-standard", "--", rel_path)
    if code == 0 and untracked.strip():
        return None

    probes: list[tuple[str, ...]] = [("diff", "-U0", "--cached", "--", rel_path)]
    if not staged_only:
        probes.append(("diff", "-U0", "--", rel_path))
        if base:
            probes.append(("diff", "-U0", f"{base}...HEAD", "--", rel_path))

    lines: set[int] = set()
    for probe in probes:
        lines.update(diff_added_lines(probe))
    return lines


def attributes(source: str) -> list[tuple[int, str, str]]:
    """Return `(line, attribute_text, kind)` for every `allow` / `expect` attribute.

    Attributes routinely span several lines, so the text is matched by bracket depth
    rather than by scanning a single line.
    """
    found: list[tuple[int, str, str]] = []
    for match in ATTR_RE.finditer(source):
        depth = 0
        end = -1
        for offset in range(match.start(), len(source)):
            char = source[offset]
            if char == "[":
                depth += 1
            elif char == "]":
                depth -= 1
                if depth == 0:
                    end = offset
                    break
        if end < 0:
            continue
        line = source.count("\n", 0, match.start()) + 1
        found.append((line, source[match.start() : end + 1], match.group(1)))
    return found


def check_file(rel_path: str, only_lines: set[int] | None = None) -> list[tuple[str, int, str]]:
    path = ROOT / rel_path
    try:
        source = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        source = path.read_text(encoding="utf-8", errors="replace")

    issues: list[tuple[str, int, str]] = []
    for token in lex(source):
        if not token.is_comment:
            continue
        if only_lines is not None and token.line not in only_lines:
            continue
        if not token.doc:
            issues.append(
                (
                    "inline",
                    token.line,
                    "inline comments are banned; move the fact to rustdoc (/// / //!) or docs/",
                )
            )
            continue

        body = comment_body(token)
        if CJK_RE.search(body):
            issues.append(("cjk", token.line, f"rustdoc must be English: {first_line(body)}"))
        if is_separator(body):
            issues.append(("separator", token.line, "decorative separator"))
        if TODO_RE.search(body):
            issues.append(("todo", token.line, "TODO/FIXME is banned in code; track it in an issue"))

    for line_no, attr, kind in attributes(source):
        if only_lines is not None and line_no not in only_lines:
            continue
        if kind == "allow" and SHARED_HARNESS_ALLOW.fullmatch(attr) and "/tests/" in rel_path:
            continue
        if kind == "allow":
            issues.append(
                (
                    "allow",
                    line_no,
                    "`#[allow(...)]` is banned; use `#[expect(..., reason = \"...\")]`",
                )
            )
        elif not REASON_RE.search(attr):
            issues.append(
                (
                    "expect",
                    line_no,
                    "`#[expect(...)]` requires `reason = \"...\"`",
                )
            )

    return issues


def markdown_row(total: int, counts: dict[str, int], warn_only: bool) -> str:
    if not total:
        return "| comment style | ✅ | clean |"
    status = "⚠️" if warn_only else "❌"
    detail = (
        f"{total} finding(s): inline {counts.get('inline', 0)}, "
        f"cjk {counts.get('cjk', 0)}, "
        f"separator {counts.get('separator', 0)}, "
        f"todo {counts.get('todo', 0)}, "
        f"allow {counts.get('allow', 0)}, "
        f"expect {counts.get('expect', 0)}"
    )
    return f"| comment style | {status} | {detail} |"


def main() -> int:
    parser = argparse.ArgumentParser(description="Check Rust comments against the repository style")
    parser.add_argument("paths", nargs="*", help="explicit files or directories; overrides selection flags")
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--all", action="store_true", help="check every tracked .rs file")
    selection.add_argument("--changed", action="store_true", help="check files changed against --base")
    selection.add_argument("--staged", action="store_true", help="check staged .rs files")
    parser.add_argument("--base", help="git ref for --changed (default: origin/master when present)")
    parser.add_argument(
        "--added-lines",
        action="store_true",
        help="only check lines added/modified in git diff (for --changed / --staged)",
    )
    parser.add_argument("--warn-only", action="store_true", help="print findings but exit 0")
    parser.add_argument("--markdown", action="store_true", help="print one Markdown row on stdout")
    parser.add_argument("--quiet", action="store_true", help="summary only")
    parser.add_argument("--max-examples", type=int, default=30, help="limit printed findings")
    args = parser.parse_args()

    base_ref: str | None = None
    if args.paths:
        paths = explicit_files(args.paths)
    elif args.staged:
        paths = staged_files()
    elif args.changed:
        base_ref = resolve_base(args.base)
        paths = changed_files(base_ref)
    elif args.all:
        paths = tracked_files()
    else:
        # Default to the useful PR behavior when git is available.
        if git("rev-parse", "--verify", "--quiet", "HEAD")[0] == 0:
            base_ref = resolve_base(args.base)
            paths = changed_files(base_ref)
        else:
            paths = tracked_files()

    if not paths:
        if args.markdown:
            print(markdown_row(0, {}, args.warn_only))
        elif not args.quiet:
            print("comments: no .rs files to check")
        return 0

    issues_by_file: list[tuple[str, list[tuple[str, int, str]]]] = []
    counts: dict[str, int] = {}
    total = 0
    for rel_path in paths:
        only_lines = (
            added_line_numbers(rel_path, base_ref, args.staged)
            if args.added_lines and not args.all and not args.paths
            else None
        )
        issues = check_file(rel_path, only_lines=only_lines)
        if not issues:
            continue
        issues_by_file.append((rel_path, issues))
        total += len(issues)
        for kind, _line, _message in issues:
            counts[kind] = counts.get(kind, 0) + 1

    if args.markdown:
        print(markdown_row(total, counts, args.warn_only))
        if total and not args.quiet:
            stream = sys.stderr
            shown = 0
            for rel_path, issues in issues_by_file:
                for kind, line, message in issues:
                    if shown >= args.max_examples:
                        break
                    print(f"{rel_path}:{line}: [{kind}] {message}", file=stream)
                    shown += 1
            if total > shown:
                print(f"comments: ... {total - shown} more finding(s)", file=stream)
    elif not args.quiet:
        shown = 0
        for rel_path, issues in issues_by_file:
            for kind, line, message in issues:
                if shown >= args.max_examples:
                    break
                print(f"{rel_path}:{line}: [{kind}] {message}")
                shown += 1
        if total > shown:
            print(f"comments: ... {total - shown} more finding(s)")

    if total:
        summary = (
            f"comments: {total} finding(s) in {len(issues_by_file)} file(s) "
            f"(inline={counts.get('inline', 0)}, cjk={counts.get('cjk', 0)}, "
            f"separator={counts.get('separator', 0)}, todo={counts.get('todo', 0)}, "
            f"allow={counts.get('allow', 0)}, expect={counts.get('expect', 0)})"
        )
        if not args.quiet:
            print(summary, file=sys.stderr if args.markdown else sys.stdout)
    elif not args.quiet and not args.markdown:
        print("comments: ok")

    return 0 if args.warn_only or not total else 1


if __name__ == "__main__":
    raise SystemExit(main())
