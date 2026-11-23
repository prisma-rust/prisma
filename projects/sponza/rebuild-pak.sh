#!/usr/bin/env bash
# Sponza scenes.pak one-click rebuild script (Bash)
#
# Usage:
#   ./rebuild-pak.sh
#   or run from repo root:
#   bash projects/sponza/rebuild-pak.sh
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
    if [ -d "$HOME/.cargo/bin" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
fi

SPONZA_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SPONZA_DIR/../.." && pwd)"

cd "$SPONZA_DIR"

echo "==> [rebuild-pak] Rebuilding Sponza scene package (assets/scenes.pak)..."
cargo run -p prism-build-pipeline --manifest-path "$ROOT_DIR/Cargo.toml" -- build-scenes "$@"
echo "==> [rebuild-pak] Successfully updated assets/scenes.pak!"
