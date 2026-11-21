#!/usr/bin/env python3
"""Install tracked git hooks (.githooks) and commit template in one command."""

from __future__ import annotations

import os
import stat
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def ensure_hook_executable(path: Path) -> None:
    """Git ignores a hook the index does not mark executable on POSIX checkouts."""
    if os.name == "nt" or not path.is_file():
        return
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def main() -> int:
    hooks = ROOT / ".githooks"
    for name in ("pre-push", "pre-push.sh", "commit-msg"):
        ensure_hook_executable(hooks / name)
    for key, value in (("core.hooksPath", ".githooks"), ("commit.template", ".gitmessage")):
        res = subprocess.run(
            ["git", "config", key, value], cwd=ROOT, text=True, encoding="utf-8", check=False
        )
        if res.returncode != 0:
            return res.returncode
    print("Installed git hooks (.githooks) and commit.template (.gitmessage).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
