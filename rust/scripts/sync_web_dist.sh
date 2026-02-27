#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKSPACE_DIR="$(cd "$RUST_DIR/.." && pwd)"
WEB_DIST_DIR="$WORKSPACE_DIR/web/dist"
EMBEDDED_DIST_DIR="$RUST_DIR/web-dist"

if [[ ! -f "$WEB_DIST_DIR/index.html" ]]; then
  echo "Missing $WEB_DIST_DIR/index.html" >&2
  echo "Build web assets first: (cd \"$WORKSPACE_DIR/web\" && npm ci && npm run build)" >&2
  exit 1
fi

rm -rf "$EMBEDDED_DIST_DIR"
mkdir -p "$EMBEDDED_DIST_DIR"
cp -R "$WEB_DIST_DIR/." "$EMBEDDED_DIST_DIR/"

if [[ ! -f "$EMBEDDED_DIST_DIR/index.html" ]]; then
  echo "Failed to sync embedded web assets into $EMBEDDED_DIST_DIR" >&2
  exit 1
fi

echo "Synced $WEB_DIST_DIR -> $EMBEDDED_DIST_DIR"
