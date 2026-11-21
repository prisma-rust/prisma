#!/usr/bin/env python3
"""Real-time GitHub issue / PR / comment event watcher using HTTP ETag (If-None-Match).

Conditional 304 Not Modified responses do not count against GitHub's API rate
limit, making this a zero-server event hook for local agents or daemons.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

WATCHED_TYPES = {
    "IssuesEvent",
    "PullRequestEvent",
    "IssueCommentEvent",
    "PullRequestReviewCommentEvent",
}


def format_event(ev: dict) -> str | None:
    kind = ev.get("type")
    if kind not in WATCHED_TYPES:
        return None
    payload = ev.get("payload", {})
    action = payload.get("action", "")
    actor = ev.get("actor", {}).get("login", "unknown")
    if kind == "IssuesEvent":
        num = payload.get("issue", {}).get("number")
        title = payload.get("issue", {}).get("title", "")
        return f"[issue:{action}] #{num} by {actor}: {title}"
    if kind == "PullRequestEvent":
        num = payload.get("pull_request", {}).get("number")
        title = payload.get("pull_request", {}).get("title", "")
        return f"[pr:{action}] #{num} by {actor}: {title}"
    if kind in ("IssueCommentEvent", "PullRequestReviewCommentEvent"):
        issue = payload.get("issue") or payload.get("pull_request") or {}
        num = issue.get("number")
        return f"[comment:{action}] #{num} by {actor}"
    return None


def gh_token() -> str | None:
    proc = subprocess.run(
        ["gh", "auth", "token"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    tok = proc.stdout.strip()
    return tok if proc.returncode == 0 and tok else None


def poll_once(repo: str, etag: str | None, token: str | None) -> tuple[list[dict], str | None, int]:
    url = f"https://api.github.com/repos/{repo}/events?per_page=30"
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "prisma-github-watch"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if etag:
        headers["If-None-Match"] = etag
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            interval = int(resp.headers.get("X-Poll-Interval", "60"))
            new_etag = resp.headers.get("ETag")
            data = json.loads(resp.read().decode("utf-8"))
            return data, new_etag, interval
    except urllib.error.HTTPError as err:
        if err.code == 304:
            interval = int(err.headers.get("X-Poll-Interval", "60"))
            return [], etag, interval
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description="ETag-based GitHub issue/PR/comment event hook")
    parser.add_argument("--repo", default="prisma-rust/prisma")
    parser.add_argument("--once", action="store_true", help="Fetch once and exit")
    parser.add_argument("--exec", dest="hook_cmd", help="Shell command invoked with event JSON on stdin")
    args = parser.parse_args()

    token = gh_token()
    seen_ids: set[str] = set()
    etag: str | None = None
    first = True

    while True:
        events, etag, interval = poll_once(args.repo, etag, token)
        if first:
            for ev in events:
                if ev_id := ev.get("id"):
                    seen_ids.add(ev_id)
            first = False
        else:
            for ev in reversed(events):
                ev_id = ev.get("id")
                if not ev_id or ev_id in seen_ids:
                    continue
                seen_ids.add(ev_id)
                line = format_event(ev)
                if line:
                    print(line, flush=True)
                    if args.hook_cmd:
                        subprocess.run(
                            args.hook_cmd,
                            shell=True,
                            input=json.dumps(ev),
                            text=True,
                            encoding="utf-8",
                            errors="replace",
                        )
        if args.once:
            return 0
        time.sleep(max(10, interval))


if __name__ == "__main__":
    raise SystemExit(main())
