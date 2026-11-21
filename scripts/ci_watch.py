#!/usr/bin/env python3
"""Wait for all GitHub Actions runs on a PR (or commit SHA) to finish and report results.

Designed to be launched once as a background command so agents wake up only when
CI completes, instead of polling with timers.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time


def run_gh(args: list[str]) -> tuple[int, str, str]:
    proc = subprocess.run(
        ["gh", *args], capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    return proc.returncode, proc.stdout, proc.stderr


def resolve_pr_head_sha(repo: str, pr: int) -> str:
    code, out, err = run_gh(["api", f"repos/{repo}/pulls/{pr}", "--jq", ".head.sha"])
    if code != 0 or not out.strip():
        raise RuntimeError(f"failed to resolve PR #{pr} head SHA: {err.strip()}")
    return out.strip()


def fetch_check_runs(repo: str, sha: str) -> list[dict]:
    code, out, err = run_gh(["api", f"repos/{repo}/commits/{sha}/check-runs"])
    if code != 0:
        raise RuntimeError(f"failed to fetch check-runs for {sha}: {err.strip()}")
    return json.loads(out).get("check_runs", [])


def all_checks_completed(checks: list[dict]) -> bool:
    return bool(checks) and all(c.get("status") == "completed" for c in checks)


def summarize_checks(checks: list[dict]) -> tuple[bool, list[str]]:
    ok = True
    lines: list[str] = []
    for c in sorted(checks, key=lambda x: x.get("name", "")):
        name = c.get("name", "unknown")
        status = c.get("status", "unknown")
        conclusion = c.get("conclusion")
        if status == "completed" and conclusion not in ("success", "skipped", "neutral"):
            ok = False
        lines.append(f"{name}: {status} ({conclusion})")
    return ok, lines


def main() -> int:
    parser = argparse.ArgumentParser(description="Wait for PR/commit CI completion")
    parser.add_argument("pr", type=int, nargs="?", help="PR number")
    parser.add_argument("--sha", help="Commit SHA (alternative to PR number)")
    parser.add_argument("--repo", default="prisma-rust/prisma", help="GitHub owner/repo")
    parser.add_argument("--interval", type=int, default=20, help="Poll interval in seconds")
    parser.add_argument("--timeout", type=int, default=1800, help="Timeout in seconds")
    args = parser.parse_args()

    if not args.pr and not args.sha:
        parser.error("provide either PR number or --sha")

    sha = args.sha or resolve_pr_head_sha(args.repo, args.pr)
    deadline = time.monotonic() + args.timeout

    while True:
        checks = fetch_check_runs(args.repo, sha)
        if all_checks_completed(checks):
            ok, lines = summarize_checks(checks)
            for line in lines:
                print(line)
            return 0 if ok else 1
        if time.monotonic() >= deadline:
            print(f"timeout waiting for {sha}", file=sys.stderr)
            for line in summarize_checks(checks)[1]:
                print(line, file=sys.stderr)
            return 2
        time.sleep(max(5, args.interval))


if __name__ == "__main__":
    raise SystemExit(main())
