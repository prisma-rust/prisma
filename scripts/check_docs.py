#!/usr/bin/env python3
"""Verify docs/*.md naming conventions (kebab-case + domain prefix) and docs/README.md registration."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS_DIR = ROOT / "docs"
EXEMPT_EXISTING = {"README.md", "DESIGN.md", "REFLECT.md", "integrations.md", "versioning.md"}
KEBAB_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)+\.md$")


def check_docs(docs_dir: Path = DOCS_DIR) -> list[str]:
    readme_path = docs_dir / "README.md"
    if not readme_path.is_file():
        return ["missing docs/README.md"]
    readme = readme_path.read_text(encoding="utf-8", errors="replace")
    errors: list[str] = []
    for path in sorted(docs_dir.glob("*.md")):
        name = path.name
        if name not in EXEMPT_EXISTING and not KEBAB_RE.match(name):
            errors.append(f"docs/{name}: expected lowercase kebab-case with domain prefix")
        if name != "README.md" and name not in readme:
            errors.append(f"docs/{name}: not registered in docs/README.md")
    return errors


def main() -> int:
    errors = check_docs()
    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        return 1
    print("docs: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
