#!/usr/bin/env python3
"""Validate that a commit subject matches docs/ai-workflow.md §3.

Called by .githooks/commit-msg; can also be run by hand:

    python scripts/check_commit_msg.py .git/COMMIT_EDITMSG
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

TYPES = ("feat", "fix", "refactor", "perf", "docs", "test", "chore", "ci", "build")
PATTERN = re.compile(
    rf"^({'|'.join(TYPES)})(\([a-z0-9][a-z0-9._/-]*\))?!?: \S"
)
ALLOWED_PREFIXES = ("Merge ", 'Revert "', "fixup! ", "squash! ", "amend! ")
MAX_SUBJECT = 72


def main() -> int:
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    if len(sys.argv) != 2:
        print("usage: check_commit_msg.py <commit-msg-file>", file=sys.stderr)
        return 2

    path = Path(sys.argv[1])
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    subject = next(
        (line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")),
        "",
    )

    if not subject:
        print("commit-msg: empty subject", file=sys.stderr)
        return 1
    if subject.startswith(ALLOWED_PREFIXES):
        return 0
    if not PATTERN.match(subject):
        print(
            "commit-msg: subject does not match the convention\n"
            "  expected: <type>(<scope>): <imperative summary>\n"
            f"  type: {' | '.join(TYPES)}\n"
            f"  actual: {subject}\n"
            "  see docs/ai-workflow.md §3",
            file=sys.stderr,
        )
        return 1
    if len(subject) > MAX_SUBJECT:
        print(
            f"commit-msg: subject is {len(subject)} characters, over the {MAX_SUBJECT} limit; "
            "move the detail into the body.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
