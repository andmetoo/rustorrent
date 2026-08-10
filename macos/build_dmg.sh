#!/bin/bash
# Compatibility wrapper for the canonical macOS packager.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ARGS=(--dmg --output "$PROJECT_DIR")

if [[ "${1:-}" == "--universal" ]]; then
    ARGS+=(--universal)
elif [[ $# -gt 0 ]]; then
    echo "unknown argument: $1" >&2
    exit 1
fi

exec "$SCRIPT_DIR/package_app.sh" "${ARGS[@]}"
