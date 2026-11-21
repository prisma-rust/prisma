#!/usr/bin/env python3
"""Print GitHub Milestones summary for prisma-rust/prisma."""

import json
import subprocess
import sys


def main() -> int:
    res = subprocess.run(
        ["gh", "api", "repos/prisma-rust/prisma/milestones?state=all"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if res.returncode != 0:
        print(res.stderr.strip(), file=sys.stderr)
        return res.returncode

    milestones = json.loads(res.stdout)
    if not milestones:
        print("No milestones found.")
        return 0

    for m in sorted(milestones, key=lambda x: x["number"]):
        open_n = m["open_issues"]
        closed_n = m["closed_issues"]
        total = open_n + closed_n
        pct = (closed_n * 100 // total) if total else 0
        print(
            f"[{m['number']}] {m['title']} ({m['state']}) — "
            f"{closed_n}/{total} closed ({pct}%) | {m['html_url']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
