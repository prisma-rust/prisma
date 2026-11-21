#!/usr/bin/env python3
"""Rewrite CRLF line endings to LF across the worktree.

    python scripts/to_lf.py                 # convert every tracked text file
    python scripts/to_lf.py --check         # report CRLF files, exit 1 if any
    python scripts/to_lf.py --dry-run       # print what would change, write nothing
    python scripts/to_lf.py --untracked     # also include untracked, non-ignored files
    python scripts/to_lf.py PATH [PATH...]  # limit the run to files or directories

The index stores LF. A Windows checkout that leaves `core.autocrlf` unset keeps
the worktree in CRLF, so the first tool that rewrites such a file turns every
line into a diff hunk even though no content changed. This script puts the
worktree back to byte-identical LF. It never stages, commits, or otherwise
mutates git state.

Binary files are skipped: git's own `ls-files --eol` classification (`w/-text`)
decides for tracked files, and a NUL byte in the first 8 KiB decides for
untracked ones.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib.console import force_utf8_streams  # noqa: E402

SNIFF_BYTES = 8192


def git(*args: str) -> tuple[int, str]:
    proc = subprocess.run(
        ["git", *args],
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return proc.returncode, proc.stdout


def git_paths(include_untracked: bool) -> list[Path]:
    """Repository-relative paths of tracked, and optionally untracked, files."""
    probes = [["ls-files", "-z"]]
    if include_untracked:
        probes.append(["ls-files", "-z", "--others", "--exclude-standard"])
    paths: list[Path] = []
    for probe in probes:
        code, out = git(*probe)
        if code != 0:
            continue
        paths.extend(ROOT / entry for entry in out.split("\0") if entry)
    return paths


def git_eol_tags() -> dict[str, str]:
    """Map path -> git's worktree EOL tag: `lf`, `crlf`, `mixed` or `-text` (binary)."""
    code, out = git("ls-files", "--eol", "-z")
    if code != 0:
        return {}
    tags: dict[str, str] = {}
    for record in out.split("\0"):
        info, _, path = record.partition("\t")
        fields = info.split()
        if len(fields) < 2 or not path:
            continue
        worktree = fields[1]
        tags[path] = worktree.split("/", 1)[1] if "/" in worktree else worktree
    return tags


def convert_bytes(data: bytes) -> bytes:
    """Return `data` with every CRLF and lone CR rewritten to LF."""
    return data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def looks_binary(data: bytes) -> bool:
    """A NUL byte near the start means the file must not be rewritten."""
    return b"\0" in data[:SNIFF_BYTES]


def convert_file(path: Path) -> bool:
    """Rewrite one file to LF; returns True when the bytes changed."""
    data = path.read_bytes()
    if b"\r" not in data or looks_binary(data):
        return False
    fixed = convert_bytes(data)
    if fixed == data:
        return False
    path.write_bytes(fixed)
    return True


def expand(explicit: list[str]) -> list[Path]:
    """Files named directly, or every file below a named directory."""
    targets: list[Path] = []
    for raw in explicit:
        candidate = (ROOT / raw).resolve()
        if candidate.is_dir():
            targets.extend(p for p in sorted(candidate.rglob("*")) if p.is_file())
        elif candidate.is_file():
            targets.append(candidate)
        else:
            raise SystemExit(f"to_lf: no such path: {raw}")
    return targets


def display(path: Path) -> str:
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)


def main() -> int:
    force_utf8_streams()
    parser = argparse.ArgumentParser(description="Rewrite CRLF line endings to LF.")
    parser.add_argument("paths", nargs="*", help="files or directories to limit the run to")
    parser.add_argument(
        "--check",
        action="store_true",
        help="report files that need LF and exit 1 if any (writes nothing)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the files that would change (writes nothing)",
    )
    parser.add_argument(
        "--untracked",
        action="store_true",
        help="also include untracked, non-ignored files",
    )
    args = parser.parse_args()

    targets = expand(args.paths) if args.paths else git_paths(args.untracked)
    tags = git_eol_tags()
    read_only = args.check or args.dry_run

    pending: list[str] = []
    converted: list[str] = []
    for path in targets:
        if ".git" in path.parts:
            continue
        if tags.get(display(path)) == "-text":
            continue
        try:
            data = path.read_bytes()
        except OSError as exc:
            print(f"to_lf: cannot read {display(path)}: {exc}", file=sys.stderr)
            continue
        if b"\r" not in data or looks_binary(data):
            continue
        name = display(path)
        pending.append(name)
        if read_only:
            continue
        if convert_file(path):
            converted.append(name)

    if read_only:
        for name in pending:
            print(name)
        if pending:
            print(
                f"to_lf: {len(pending)} file(s) need LF — run `python scripts/to_lf.py`",
                file=sys.stderr,
            )
            return 1
        print("to_lf: every file already uses LF")
        return 0

    if converted:
        print(f"to_lf: converted {len(converted)} file(s) to LF")
    else:
        print("to_lf: every file already uses LF")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
