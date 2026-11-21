#!/usr/bin/env bash
# Convert CRLF line endings to LF in the worktree.
#
#   scripts/to_lf.sh                # convert every tracked text file
#   scripts/to_lf.sh --check        # report CRLF files, exit 1 if any
#   scripts/to_lf.sh --dry-run      # print what would change, write nothing
#   scripts/to_lf.sh PATH ...       # limit the run to files or directories
#
# Thin wrapper: the implementation is scripts/to_lf.py. Set PYTHON to force an
# interpreter.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

python_bin="${PYTHON:-}"
if [ -z "$python_bin" ]; then
  for candidate in python3 python py; do
    if command -v "$candidate" >/dev/null 2>&1; then
      python_bin="$candidate"
      break
    fi
  done
fi
if [ -z "$python_bin" ]; then
  echo "to_lf: no python interpreter found; set PYTHON." >&2
  exit 1
fi

exec "$python_bin" "$HERE/to_lf.py" "$@"
