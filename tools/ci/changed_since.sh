#!/usr/bin/env bash
# Single source of truth for change-gated CI & nightly workflows.
# Usage: changed_since.sh <base> <tip> [commits|version]
# Exit 0 = changes require running; Exit 1 = safe to skip.
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    echo "usage: $0 <base> <tip> [commits|version]" >&2
    exit 2
fi

base=$1
tip=$2
scope=${3:-commits}

case "$scope" in
    commits)
        changed=$(git diff --name-only "$base" "$tip")
        if [ -z "$changed" ]; then
            exit 1
        fi
        while IFS= read -r path; do
            [ -n "$path" ] || continue
            case "$path" in
                docs/*|*.md) ;;
                *) exit 0 ;;
            esac
        done <<< "$changed"
        exit 1
        ;;
    version)
        if git diff -U0 "$base" "$tip" -- Cargo.toml | grep -qE '^[+-]version[[:space:]]*='; then
            exit 0
        fi
        exit 1
        ;;
    *)
        echo "unknown scope: $scope (expected commits|version)" >&2
        exit 2
        ;;
esac
