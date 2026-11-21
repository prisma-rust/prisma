#!/usr/bin/env python3
"""Install tracked git hooks (.githooks) and commit template in one command."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
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
